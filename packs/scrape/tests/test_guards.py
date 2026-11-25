# SPDX-License-Identifier: Apache-2.0
"""Hermetic tests for the extractor's guards (no network, no trafilatura)."""
import os
import socket
import sys
import threading
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import praxi_scrape as s  # noqa: E402


class AddressGuard(unittest.TestCase):
    def test_private_loopback_metadata_and_mapped_addresses_are_refused(self):
        for bad in ["127.0.0.1", "10.1.2.3", "192.168.0.9", "169.254.169.254", "100.64.0.1",
                    "0.0.0.0", "::1", "fd00:ec2::254", "fe80::1", "::ffff:127.0.0.1", "224.0.0.1"]:
            self.assertFalse(s.is_public_ip(bad), bad)
        self.assertTrue(s.is_public_ip("93.184.216.34"))

    def test_a_name_that_resolves_to_a_private_address_is_refused_even_when_mixed(self):
        def resolver(_h, _p, type=0):
            return [(2, 1, 6, "", ("93.184.216.34", 80)), (2, 1, 6, "", ("10.0.0.5", 80))]
        with self.assertRaises(s.Refused) as c:
            s.resolve_public("example.test", 80, resolver)
        self.assertEqual(c.exception.code, "blocked_address")

    def test_ip_literals_and_odd_urls_are_refused(self):
        for url, code in [("http://127.0.0.1/", "blocked_address"), ("http://[::1]/", "blocked_address"),
                          ("http://169.254.169.254/latest/meta-data", "blocked_address"),
                          ("file:///etc/passwd", "bad_request"), ("ftp://example.com/", "bad_request"),
                          ("http://user:pw@example.com/", "bad_request"),
                          ("http://example.com:22/", "blocked_address")]:
            with self.assertRaises(s.Refused) as c:
                parts = s.check_url(url)
                s.resolve_public(parts.hostname, parts.port or 80)
            self.assertEqual(c.exception.code, code, url)

    def test_every_redirect_hop_is_checked_again(self):
        opened = []

        class Resp:
            def __init__(self, path):
                self.status, self.path = (404, path) if path.endswith("robots.txt") else (302, path)

            def getheaders(self):
                return [("Location", "http://127.0.0.1/admin")] if self.status == 302 else []

            def read1(self, _n):
                return b""

        class Conn:
            def __init__(self, host):
                self.host = host

            def request(self, _m, path, headers=None):
                self.path = path

            def getresponse(self):
                return Resp(self.path)

            def close(self):
                pass

        def opener(parts, ip, timeout, track=None):
            opened.append(ip)
            return Conn(parts.hostname)

        orig = s.socket.getaddrinfo
        s.socket.getaddrinfo = lambda h, p, type=0: [(2, 1, 6, "", ("93.184.216.34", p))]
        try:
            with self.assertRaises(s.Refused) as c:
                s.fetch_following("http://example.test/x", time.monotonic() + 5, s.RateLimiter(0, 30), {},
                                  lambda u, d, m=s.MAX_BYTES: s.fetch_once(u, d, m, opener))
        finally:
            s.socket.getaddrinfo = orig
        self.assertEqual(c.exception.code, "blocked_address")
        self.assertEqual(set(opened), {"93.184.216.34"})  # loopback was never connected to


class Robots(unittest.TestCase):
    BODY = "User-agent: *\nDisallow: /private\nCrawl-delay: 2\n\nUser-agent: PraxiBot\nDisallow: /nobots\nCrawl-delay: 3\n"

    def test_disallow_and_crawl_delay(self):
        self.assertEqual(s.robots_decision(200, self.BODY, "/nobots/a"), ("blocked", 0.0))
        self.assertEqual(s.robots_decision(200, self.BODY, "/ok"), ("allowed", 3.0))

    def test_status_rules(self):
        self.assertEqual(s.robots_decision(404, "", "/x")[0], "allowed")
        self.assertEqual(s.robots_decision(503, "", "/x")[0], "blocked")
        self.assertEqual(s.robots_decision(None, "", "/x")[0], "unknown")
        self.assertEqual(s.robots_decision(200, "User-agent: *\nCrawl-delay: 600\n", "/x")[0], "blocked")


class Pacing(unittest.TestCase):
    def test_spacing_per_site_and_minute_budget(self):
        rl = s.RateLimiter(1.0, 3)
        self.assertEqual(rl.reserve("a", 100.0), 0.0)
        self.assertAlmostEqual(rl.reserve("a", 100.0), 1.0)
        self.assertEqual(rl.reserve("b", 100.0), 0.0)  # other sites are independent
        self.assertAlmostEqual(rl.reserve("a", 100.0, extra_interval=5.0), 6.0)  # robots Crawl-delay widens the gap
        self.assertIsNone(rl.reserve("a", 100.5))  # 3 per minute spent
        self.assertEqual(rl.reserve("a", 161.0), 0.0)  # a minute later it is open again

    def test_the_robots_txt_fetch_uses_the_same_per_site_budget(self):
        calls = []

        def fake(url, deadline, max_bytes=s.MAX_BYTES):
            calls.append(url)
            return 404, {}, b"", False

        orig = s.resolve_public
        s.resolve_public = lambda host, port, resolver=None: ["93.184.216.34"]
        try:
            with self.assertRaises(s.Refused) as c:  # budget of ONE request: robots.txt spends it
                s.fetch_following("http://example.test/a", time.monotonic() + 5, s.RateLimiter(0, 1), {}, fake)
        finally:
            s.resolve_public = orig
        self.assertEqual(c.exception.code, "rate_limited")
        self.assertEqual(calls, ["http://example.test/robots.txt"])


class Auth(unittest.TestCase):
    def test_token_is_required_and_bind_is_loopback(self):
        import http.client
        import http.server
        h = s.make_handler("t" * 40, s.RateLimiter(), {}, threading.BoundedSemaphore(2))
        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), h)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            def post(tok):
                c = http.client.HTTPConnection("127.0.0.1", srv.server_address[1], timeout=5)
                hdr = {"Content-Type": "application/json"}
                if tok:
                    hdr["X-Praxi-Token"] = tok
                c.request("POST", "/v1/extract", b'{"url":"http://127.0.0.1/"}', hdr)
                r = c.getresponse()
                return r.status, r.read()
            self.assertEqual(post(None)[0], 401)
            self.assertEqual(post("wrong")[0], 401)
            status, body = post("t" * 40)  # right token: the SSRF guard answers, not a crash
            self.assertEqual(status, 200)
            self.assertIn(b"blocked_address", body)
        finally:
            srv.shutdown()
            srv.server_close()
        self.assertEqual(s.BIND_HOST, "127.0.0.1")


class Pipeline(unittest.TestCase):
    def test_cap_and_untrusted_markup_are_handled(self):
        def fake(url, deadline, max_bytes=s.MAX_BYTES):
            if url.endswith("/robots.txt"):
                return 404, {}, b"", False
            return 200, {"content-type": "text/html; charset=utf-8"}, b"<title>T</title><a href='/b'>B</a>", False
        orig = s.socket.getaddrinfo
        s.socket.getaddrinfo = lambda h, p, type=0: [(2, 1, 6, "", ("93.184.216.34", p))]
        try:
            out = s.run_request({"url": "http://example.test/a", "max_chars": 200},
                                s.RateLimiter(0, 30), {}, fake, convert=lambda t, u, m: "x" * 500)
        finally:
            s.socket.getaddrinfo = orig
        self.assertEqual(len(out["markdown"]), 200)
        self.assertTrue(out["truncated"])
        self.assertEqual(out["title"], "T")
        self.assertEqual(out["links"][0]["url"], "http://example.test/b")


class TimeCap(unittest.TestCase):
    """The deadline is a wall-clock cap, not "checked between reads": a page that keeps
    sending one byte at a time must not keep the handler thread (or a slot) for longer."""

    def _fetch_slowly(self, opener, budget=1.0, url="http://example.test:8080/x"):
        """Run fetch_once on a worker thread; a hang (the old behaviour) shows up as `alive`."""
        out = {}
        orig = s.resolve_public
        s.resolve_public = lambda host, port, resolver=None: ["93.184.216.34"]

        def run():
            started = time.monotonic()
            try:
                out["ok"] = s.fetch_once(url, started + budget, s.MAX_BYTES, opener)
            except s.Refused as e:
                out["code"] = e.code
            out["took"] = time.monotonic() - started

        th = threading.Thread(target=run, daemon=True)
        th.start()
        th.join(budget + 3.0)
        s.resolve_public = orig
        return th, out

    def _drip_server(self, head: bytes, drip: bytes):
        """Loopback server: sends `head`, then `drip` one byte per 0.1 s until the peer is gone."""
        srv = socket.socket()
        srv.bind(("127.0.0.1", 0))
        srv.listen(4)
        stop = threading.Event()

        def serve():
            srv.settimeout(0.2)
            while not stop.is_set():
                try:
                    c, _ = srv.accept()
                except OSError:
                    continue
                c.settimeout(5)
                try:
                    c.recv(4096)
                    c.sendall(head)
                    while not stop.is_set():
                        c.sendall(drip)
                        time.sleep(0.1)
                except OSError:
                    pass
                finally:
                    c.close()

        threading.Thread(target=serve, daemon=True).start()
        self.addCleanup(lambda: (stop.set(), srv.close()))
        return srv.getsockname()[1]

    def _real_opener(self, port):
        def opener(parts, ip, timeout, track=None):
            conn = s._open(s.urllib.parse.urlsplit(f"http://example.test:{port}/"), "127.0.0.1", timeout)
            if track:
                track(conn.sock)
            return conn
        return opener

    def test_a_body_that_drips_one_byte_at_a_time_hits_the_deadline(self):
        port = self._drip_server(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n", b"x")
        th, out = self._fetch_slowly(self._real_opener(port))
        self.assertFalse(th.is_alive(), "fetch_once is still blocked past its deadline")
        self.assertEqual(out.get("code"), "timeout", out)
        self.assertLess(out["took"], 3.0)

    def test_headers_that_drip_forever_hit_the_deadline(self):
        port = self._drip_server(b"HTTP/1.1 200 OK\r\nX-Slow: ", b"a")
        th, out = self._fetch_slowly(self._real_opener(port))
        self.assertFalse(th.is_alive(), "fetch_once is still blocked in the header phase")
        self.assertEqual(out.get("code"), "timeout", out)

    def test_every_read_is_armed_with_a_short_socket_timeout(self):
        timeouts = []

        class Sock:
            def settimeout(self, t):
                timeouts.append(t)

        class Resp:
            status = 200

            def getheaders(self):
                return []

            def read1(self, _n):
                time.sleep(0.15)
                return b"x"

        class Conn:
            sock = Sock()

            def request(self, *_a, **_k):
                pass

            def getresponse(self):
                return Resp()

            def close(self):
                pass

        th, out = self._fetch_slowly(lambda parts, ip, timeout, track=None: Conn(), budget=1.0)
        self.assertEqual(out.get("code"), "timeout", out)
        self.assertGreater(len(timeouts), 3)
        self.assertTrue(all(0.1 <= t <= 5.0 for t in timeouts), timeouts)
        self.assertLessEqual(min(timeouts), 0.5)  # shrinks toward the deadline instead of staying at 20 s


if __name__ == "__main__":
    unittest.main()
