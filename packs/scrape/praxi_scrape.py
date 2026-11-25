#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Praxi web scraping add-on: a small, careful page extractor on 127.0.0.1.

Contract (also documented in docs/guide/web-scraping.md):

    POST /v1/extract   header X-Praxi-Token: <per-install token>
    {"url", "mode": "article"|"markdown"|"links", "max_chars", "timeout_ms", "max_pages"}
 -> {"status", "final_url", "title", "markdown", "links": [], "truncated",
     "robots": "allowed"|"blocked"|"unknown", "fetched_at", "pages", "error"?}
    GET /health        -> {"ok": true, "version": "..."}   (no token, no page access)

WE do the fetching (not the extraction library), so every safety rule lives in this file:
public addresses only (every redirect hop re-checked, the connection goes to the address we
checked), 5 redirects, 5 MB, 20 s, html/xml/text only, no cookies, no JavaScript, no login,
robots.txt respected, polite per-site rate limit, at most 2 pages in flight.

Standard library only, except the extraction step (trafilatura, imported lazily), so the
guards below are unit-tested without it.
"""
from __future__ import annotations

import hmac
import html.parser
import http.client
import http.server
import ipaddress
import json
import os
import socket
import ssl
import sys
import threading
import time
import urllib.parse
import urllib.robotparser
from collections import deque
from datetime import datetime, timezone

VERSION = os.environ.get("PRAXI_SCRAPE_VERSION", "1")
PROJECT_URL = "https://github.com/emreocell/Praxi-Bot"
USER_AGENT = f"PraxiBot/{VERSION} (+{PROJECT_URL})"
ROBOTS_TOKEN = "PraxiBot"

BIND_HOST = "127.0.0.1"
DEFAULT_PORT = 47811
MAX_BYTES = 5 * 1024 * 1024
MAX_REDIRECTS = 5
MAX_REQUEST_BYTES = 4096
FETCH_TIMEOUT = 20.0
MAX_PAGES = 10
MAX_CHARS_CAP = 200_000
MAX_LINKS = 200
ALLOWED_PORTS = {80, 443, 8080, 8443}
ALLOWED_TYPES = {"text/html", "application/xhtml+xml", "text/plain", "text/xml", "application/xml"}
MIN_INTERVAL = 1.0  # seconds between two requests to one site
PER_MINUTE = 30  # requests per site per minute
MAX_CRAWL_DELAY = 30.0  # a robots.txt Crawl-delay above this counts as "do not crawl"
MAX_CONCURRENT = 2
ROBOTS_TTL = 3600.0


class Refused(Exception):
    """A request we will not make. `code` is the stable error code returned to the caller."""

    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


# ---------------------------------------------------------------------------------- guards


def is_public_ip(text: str) -> bool:
    """True only for globally routable unicast addresses (no loopback, private, link-local,
    cloud metadata, carrier-grade NAT, multicast, reserved). IPv4-mapped IPv6 is unwrapped."""
    try:
        ip = ipaddress.ip_address(text.split("%")[0])
    except ValueError:
        return False
    if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped is not None:
        ip = ip.ipv4_mapped
    return ip.is_global and not ip.is_multicast


def check_url(url: str) -> urllib.parse.SplitResult:
    """Syntax checks that need no network: http(s), a host, no credentials, a normal port."""
    if len(url) > 2048:
        raise Refused("bad_request", "The address is too long.")
    parts = urllib.parse.urlsplit(url.strip())
    if parts.scheme not in ("http", "https"):
        raise Refused("bad_request", "Only http and https addresses can be read.")
    if not parts.hostname:
        raise Refused("bad_request", "The address has no host.")
    if parts.username or parts.password:
        raise Refused("bad_request", "Addresses with a login are not read.")
    try:
        port = parts.port or (443 if parts.scheme == "https" else 80)
    except ValueError:
        raise Refused("bad_request", "The address has a bad port.") from None
    if port not in ALLOWED_PORTS:
        raise Refused("blocked_address", "That port is not allowed.")
    return parts


def resolve_public(host: str, port: int, resolver=None) -> list[str]:
    """Resolve `host`; refuse unless EVERY answer is public (a mixed answer is a rebinding trick)."""
    if is_ip_literal(host) and not is_public_ip(host):
        raise Refused("blocked_address", "That address is not on the public internet.")
    try:
        infos = (resolver or socket.getaddrinfo)(host, port, type=socket.SOCK_STREAM)
    except OSError:
        raise Refused("fetch_failed", "The site could not be found.") from None
    addrs = []
    for info in infos:
        addr = info[4][0]
        if not is_public_ip(addr):
            raise Refused("blocked_address", "That address is not on the public internet.")
        if addr not in addrs:
            addrs.append(addr)
    if not addrs:
        raise Refused("fetch_failed", "The site could not be found.")
    return addrs


def is_ip_literal(host: str) -> bool:
    try:
        ipaddress.ip_address(host.strip("[]").split("%")[0])
        return True
    except ValueError:
        return False


def next_hop(current: str, location: str) -> str:
    """Absolute URL of a redirect. The caller runs the full guard on it again."""
    return urllib.parse.urljoin(current, location)


class RateLimiter:
    """Per-site pacing: at least `min_interval` between requests and `per_minute` per minute.

    `reserve` returns the seconds to wait before the request may start, or None when the
    per-minute budget is spent (the caller answers "rate_limited" instead of queueing).
    """

    def __init__(self, min_interval: float = MIN_INTERVAL, per_minute: int = PER_MINUTE):
        self.min_interval = min_interval
        self.per_minute = per_minute
        self._lock = threading.Lock()
        self._starts: dict[str, deque] = {}

    def reserve(self, site: str, now: float, extra_interval: float = 0.0) -> float | None:
        gap = max(self.min_interval, extra_interval)
        with self._lock:
            q = self._starts.setdefault(site, deque())
            while q and now - q[0] >= 60.0:
                q.popleft()
            if len(q) >= self.per_minute:
                return None
            start = max(now, q[-1] + gap) if q else now
            if start - now > 20.0:
                return None
            q.append(start)
            return start - now


def robots_decision(status: int | None, body: str, url: str) -> tuple[str, float]:
    """(robots, crawl_delay) for `url` given the site's robots.txt answer.

    RFC 9309: a missing file (4xx) allows everything; a server error means "assume
    disallowed"; no answer at all is `unknown` (the page fetch then fails by itself).
    """
    if status is None:
        return "unknown", 0.0
    if status >= 500:
        return "blocked", 0.0
    if status >= 400:
        return "allowed", 0.0
    rp = urllib.robotparser.RobotFileParser()
    rp.parse(body.splitlines())
    path = urllib.parse.urlsplit(url)
    target = path.path or "/"
    if path.query:
        target += "?" + path.query
    if not rp.can_fetch(ROBOTS_TOKEN, target):
        return "blocked", 0.0
    delay = rp.crawl_delay(ROBOTS_TOKEN) or 0
    if float(delay) > MAX_CRAWL_DELAY:
        return "blocked", 0.0
    return "allowed", float(delay)


# --------------------------------------------------------------------------------- fetching


def _kill(sock) -> None:
    """Wake any thread blocked on `sock` (shutdown works from another thread, close does not).

    Uses the plain socket method on purpose: SSLSocket.shutdown would unwrap the TLS object
    underneath a reader that is still using it."""
    try:
        if isinstance(sock, socket.socket):
            socket.socket.shutdown(sock, socket.SHUT_RDWR)
        else:
            sock.shutdown(socket.SHUT_RDWR)
    except (OSError, ValueError, AttributeError):
        pass


class _Watchdog:
    """Hard wall-clock cap for ONE fetch (TLS handshake, headers, chunk framing and body).

    Socket timeouts only fire when NO byte arrives for a while, so a server that drips one
    byte every few seconds would otherwise keep our thread (and one of the MAX_CONCURRENT
    slots) for as long as it likes. At the deadline this shuts the tracked sockets down;
    the blocked read returns and `expired` tells the caller that it was the deadline.
    """

    def __init__(self, deadline: float):
        self.expired = threading.Event()
        self._socks: list = []
        self._lock = threading.Lock()
        self._timer = threading.Timer(max(0.0, deadline - time.monotonic()), self._fire)
        self._timer.daemon = True
        self._timer.start()

    def track(self, sock) -> None:
        with self._lock:
            self._socks.append(sock)
            if self.expired.is_set():
                _kill(sock)

    def _fire(self) -> None:
        with self._lock:
            self.expired.set()
            for sock in self._socks:
                _kill(sock)

    def cancel(self) -> None:
        self._timer.cancel()


def _open(parts: urllib.parse.SplitResult, ip: str, timeout: float, track=None) -> http.client.HTTPConnection:
    """Connect to the ADDRESS we already checked (no second DNS lookup), TLS against the name.

    `track` (the watchdog) gets every socket before it can block on the peer, the TLS
    handshake included."""
    port = parts.port or (443 if parts.scheme == "https" else 80)
    sock = socket.create_connection((ip, port), timeout=timeout)
    try:
        if parts.scheme == "https":
            ctx = ssl.create_default_context()
            sock = ctx.wrap_socket(sock, server_hostname=parts.hostname, do_handshake_on_connect=False)
            if track:
                track(sock)
            sock.do_handshake()
        elif track:
            track(sock)
    except BaseException:
        sock.close()
        raise
    conn = http.client.HTTPConnection(parts.hostname, port, timeout=timeout)
    conn.sock = sock
    return conn


def _arm(conn, deadline: float) -> None:
    """Before every blocking step: stop at the deadline, and never wait more than 5 s for one read."""
    left = deadline - time.monotonic()
    if left <= 0:
        raise Refused("timeout", "The page took too long.")
    sock = getattr(conn, "sock", None)
    if sock is not None:
        sock.settimeout(max(0.1, min(5.0, left)))


def fetch_once(url: str, deadline: float, max_bytes: int = MAX_BYTES, opener=_open):
    """One GET without redirects. Returns (status, headers, body bytes, truncated).

    `deadline` (time.monotonic) is a hard cap on the whole call, not only between reads."""
    parts = check_url(url)
    port = parts.port or (443 if parts.scheme == "https" else 80)
    ip = resolve_public(parts.hostname, port)[0]
    left = deadline - time.monotonic()
    if left <= 0:
        raise Refused("timeout", "The page took too long.")
    dog = _Watchdog(deadline)
    conn = None
    try:
        conn = opener(parts, ip, min(left, FETCH_TIMEOUT), dog.track)
        target = parts.path or "/"
        if parts.query:
            target += "?" + parts.query
        _arm(conn, deadline)
        conn.request(
            "GET",
            target,
            headers={
                "Host": parts.netloc,
                "User-Agent": USER_AGENT,
                "Accept": "text/html,application/xhtml+xml,text/plain;q=0.8,*/*;q=0.1",
                "Accept-Encoding": "identity",  # no compressed bombs
                "Connection": "close",
            },
        )
        _arm(conn, deadline)
        resp = conn.getresponse()
        headers = {k.lower(): v for k, v in resp.getheaders()}
        body = bytearray()
        truncated = False
        while True:
            _arm(conn, deadline)
            chunk = resp.read1(65536)  # what one recv gave us; read(n) would wait for n bytes
            if not chunk:
                break
            body += chunk
            if len(body) > max_bytes:
                del body[max_bytes:]
                truncated = True
                break
        if dog.expired.is_set():  # the watchdog cut the connection: an empty read, not "end of page"
            raise Refused("timeout", "The page took too long.")
        return resp.status, headers, bytes(body), truncated
    except Refused:
        raise
    except (socket.timeout, TimeoutError):
        raise Refused("timeout", "The page took too long.") from None
    except (OSError, http.client.HTTPException, ssl.SSLError):
        if dog.expired.is_set():
            raise Refused("timeout", "The page took too long.") from None
        raise Refused("fetch_failed", "The page could not be loaded.") from None
    finally:
        dog.cancel()
        if conn is not None:
            conn.close()


def _pace(limiter: RateLimiter, site: str, deadline: float, extra: float = 0.0) -> None:
    """Wait for this site's turn (robots.txt fetches included); never past the overall deadline."""
    wait = limiter.reserve(site, time.monotonic(), extra)
    if wait is None:
        raise Refused("rate_limited", "Too many requests to this site. Try again in a minute.")
    if wait:
        if time.monotonic() + wait >= deadline:
            raise Refused("timeout", "The page took too long.")
        time.sleep(min(wait, 20.0))


def fetch_following(url: str, deadline: float, limiter: RateLimiter, robots_cache, fetch=fetch_once):
    """GET with redirects: robots + pacing + the full guard on EVERY hop.

    Returns (final_url, status, headers, body, truncated, robots).
    """
    current = url
    robots = "unknown"
    for hop in range(MAX_REDIRECTS + 1):
        parts = check_url(current)
        robots, delay = robots_for(parts, deadline, robots_cache, fetch, limiter)
        if robots == "blocked":
            raise Refused("robots_blocked", "The site's robots.txt asks robots not to read this page.")
        _pace(limiter, parts.netloc.lower(), deadline, delay)
        status, headers, body, truncated = fetch(current, deadline)
        if 300 <= status < 400 and headers.get("location"):
            if hop == MAX_REDIRECTS:
                raise Refused("fetch_failed", "The page redirects too many times.")
            current = next_hop(current, headers["location"])
            continue
        return current, status, headers, body, truncated, robots
    raise Refused("fetch_failed", "The page redirects too many times.")


def robots_for(parts, deadline, cache, fetch=fetch_once, limiter=None):
    origin = f"{parts.scheme}://{parts.netloc.lower()}"
    now = time.monotonic()
    hit = cache.get(origin)
    if hit is None or now - hit[0] > ROBOTS_TTL:
        if limiter is not None:  # the robots.txt request is a request to the site like any other
            _pace(limiter, parts.netloc.lower(), deadline)
        try:
            status, _h, body, _t = fetch(origin + "/robots.txt", deadline, 512 * 1024)
            text = body.decode("utf-8", "replace")
        except Refused as e:
            if e.code in ("blocked_address", "timeout"):
                raise
            status, text = None, ""
        hit = (now, status, text)
        cache[origin] = hit
    target = urllib.parse.urlunsplit(("", "", parts.path or "/", parts.query, ""))
    return robots_decision(hit[1], hit[2], target)


# -------------------------------------------------------------------------------- extraction


class _Links(html.parser.HTMLParser):
    def __init__(self, base: str):
        super().__init__(convert_charrefs=True)
        self.base = base
        self.out: list[dict] = []
        self._href: str | None = None
        self._text: list[str] = []
        self.title = ""
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self._href = dict(attrs).get("href")
            self._text = []
        elif tag == "title":
            self._in_title = True

    def handle_endtag(self, tag):
        if tag == "title":
            self._in_title = False
        if tag == "a" and self._href is not None:
            url = urllib.parse.urljoin(self.base, self._href.strip())
            if url.startswith(("http://", "https://")) and len(self.out) < MAX_LINKS:
                url = url.split("#")[0]
                if url and all(x["url"] != url for x in self.out):
                    self.out.append({"text": " ".join("".join(self._text).split())[:200], "url": url})
            self._href = None

    def handle_data(self, data):
        if self._in_title:
            self.title += data
        if self._href is not None:
            self._text.append(data)


def page_links(html_text: str, base: str) -> tuple[str, list[dict]]:
    p = _Links(base)
    try:
        p.feed(html_text)
    except Exception:  # malformed markup: keep what we have
        pass
    return " ".join(p.title.split())[:300], p.out


def decode(body: bytes, content_type: str) -> str:
    charset = "utf-8"
    for piece in content_type.split(";")[1:]:
        k, _, v = piece.strip().partition("=")
        if k.lower() == "charset" and v:
            charset = v.strip("\"'")
    try:
        return body.decode(charset, "replace")
    except LookupError:
        return body.decode("utf-8", "replace")


def to_markdown(html_text: str, url: str, mode: str) -> str:
    if mode == "links":
        return ""
    import trafilatura  # lazy: only the real service needs it

    out = trafilatura.extract(
        html_text,
        url=url,
        output_format="markdown",
        include_links=True,
        include_tables=True,
        include_comments=False,
        favor_recall=(mode == "markdown"),
    )
    return out or ""


def extract_page(url, mode, deadline, limiter, cache, fetch=fetch_once, convert=to_markdown):
    final, status, headers, body, truncated, robots = fetch_following(url, deadline, limiter, cache, fetch)
    ctype = headers.get("content-type", "text/html").split(";")[0].strip().lower()
    if status >= 400:
        raise Refused("fetch_failed", f"The site answered with an error ({status}).")
    if ctype not in ALLOWED_TYPES:
        raise Refused("bad_content_type", "That is not a web page (it is a file or media).")
    text = decode(body, headers.get("content-type", ""))
    title, links = page_links(text, final) if ctype != "text/plain" else ("", [])
    md = text if ctype == "text/plain" and mode != "links" else convert(text, final, mode)
    return {
        "status": status,
        "final_url": final,
        "title": title,
        "markdown": md,
        "links": links,
        "truncated": truncated,
        "robots": robots,
    }


def run_request(req: dict, limiter: RateLimiter, cache: dict, fetch=fetch_once, convert=to_markdown) -> dict:
    mode = req.get("mode", "article")
    if mode not in ("article", "markdown", "links"):
        raise Refused("bad_request", "Unknown mode.")
    max_chars = min(max(int(req.get("max_chars", 20000)), 200), MAX_CHARS_CAP)
    timeout = min(max(int(req.get("timeout_ms", 20000)), 1000), 60000) / 1000.0
    pages = min(max(int(req.get("max_pages", 1)), 1), MAX_PAGES)
    deadline = time.monotonic() + timeout
    first = extract_page(req["url"], mode, deadline, limiter, cache, fetch, convert)
    results = [first]
    if pages > 1:
        host = urllib.parse.urlsplit(first["final_url"]).netloc.lower()
        seen = {first["final_url"]}
        queue = [x["url"] for x in first["links"] if urllib.parse.urlsplit(x["url"]).netloc.lower() == host]
        while queue and len(results) < pages and time.monotonic() < deadline:
            nxt = queue.pop(0)
            if nxt in seen:
                continue
            seen.add(nxt)
            try:
                page = extract_page(nxt, mode, deadline, limiter, cache, fetch, convert)
            except Refused:
                continue  # one bad sub-page does not spoil the rest
            seen.add(page["final_url"])
            results.append(page)
    parts, total = [], 0
    truncated = any(p["truncated"] for p in results)
    for p in results:
        chunk = p["markdown"]
        if len(results) > 1 and chunk:
            chunk = f"## {p['title'] or p['final_url']}\n<{p['final_url']}>\n\n{chunk}"
        if total + len(chunk) > max_chars:
            chunk = chunk[: max(0, max_chars - total)]
            truncated = True
        parts.append(chunk)
        total += len(chunk)
    links, seen_l = [], set()
    for p in results:
        for link in p["links"]:
            if link["url"] not in seen_l and len(links) < MAX_LINKS:
                seen_l.add(link["url"])
                links.append(link)
    return {
        "status": first["status"],
        "final_url": first["final_url"],
        "title": first["title"],
        "markdown": "\n\n".join(x for x in parts if x),
        "links": links,
        "truncated": truncated,
        "robots": first["robots"],
        "fetched_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "pages": len(results),
    }


# ------------------------------------------------------------------------------------ server


def failure(code: str, message: str) -> dict:
    return {
        "status": 0, "final_url": "", "title": "", "markdown": "", "links": [], "truncated": False,
        "robots": "blocked" if code == "robots_blocked" else "unknown",
        "fetched_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "pages": 0, "error": code, "message": message,
    }


def make_handler(token: str, limiter: RateLimiter, cache: dict, slots: threading.BoundedSemaphore):
    class Handler(http.server.BaseHTTPRequestHandler):
        server_version = "praxi-scrape"
        sys_version = ""

        def log_message(self, *_a):  # never log URLs or page content
            pass

        def _send(self, code: int, body: dict):
            data = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/health":
                self._send(200, {"ok": True, "version": VERSION})
            else:
                self._send(404, {"error": "not_found"})

        def do_POST(self):
            if self.path != "/v1/extract":
                return self._send(404, {"error": "not_found"})
            given = self.headers.get("X-Praxi-Token", "")
            if not token or not hmac.compare_digest(given.encode(), token.encode()):
                return self._send(401, {"error": "unauthorized"})
            try:
                n = int(self.headers.get("Content-Length", "0"))
                if not 0 < n <= MAX_REQUEST_BYTES:
                    raise ValueError
                req = json.loads(self.rfile.read(n))
                if not isinstance(req, dict) or not isinstance(req.get("url"), str):
                    raise ValueError
            except (ValueError, json.JSONDecodeError):
                return self._send(400, {"error": "bad_request"})
            if not slots.acquire(timeout=1.0):
                return self._send(429, failure("busy", "The add-on is busy. Try again in a moment."))
            try:
                self._send(200, run_request(req, limiter, cache))
            except Refused as e:
                self._send(429 if e.code == "rate_limited" else 200, failure(e.code, e.message))
            except Exception:  # never leak internals
                self._send(200, failure("extract_failed", "The page could not be read."))
            finally:
                slots.release()

    return Handler


def read_token() -> str:
    path = os.environ.get("PRAXI_SCRAPE_TOKEN_FILE") or os.path.join(
        os.environ.get("CREDENTIALS_DIRECTORY", ""), "token"
    )
    with open(path, encoding="utf-8") as f:
        return f.read().strip()


def main() -> int:
    token = read_token()
    if len(token) < 32:
        print("praxi-scrape: token missing or too short", file=sys.stderr)
        return 2
    port = int(os.environ.get("PRAXI_SCRAPE_PORT", DEFAULT_PORT))
    handler = make_handler(token, RateLimiter(), {}, threading.BoundedSemaphore(MAX_CONCURRENT))
    server = http.server.ThreadingHTTPServer((BIND_HOST, port), handler)  # loopback only, never configurable
    server.daemon_threads = True
    server.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
