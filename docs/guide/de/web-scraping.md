# Web-Scraping-Zusatz (optional)

Die eingebauten [Web-Werkzeuge](../web.md) lesen die meisten Seiten bereits. Der
**Web-Scraping-Zusatz** ist ein kleines Extra-Programm für einen Server: sauberer
Artikeltext, die Linkliste einer Seite oder ein kurzer Durchlauf (bis zu 10
Seiten einer Website). Er fügt ein Werkzeug hinzu, `web_scrape`. Bots probieren
zuerst `web_extract` und nutzen `web_scrape` nur, wenn das nicht reicht.

Er ist **nicht** für Seiten mit Anmeldung gedacht und führt kein JavaScript aus.
Er meldet sich nie an, löst keine CAPTCHAs, versteckt nicht, wer er ist, und
umgeht keine Bezahlschranke. Sagt eine Website Nein, hört der Bot auf und sagt das.

## Hinzufügen oder entfernen

Das können nur Sie, von Ihrem eigenen Computer aus über Ihren SSH-Zugang. Der
Praxi-Benutzer auf dem Server hat keine Administratorrechte, ein Bot kann den
Zusatz also nie installieren.

1. **Einstellungen → Computer** öffnen, den Server wählen und **Werkzeuge** drücken.
2. **Web-Scraping** anhaken. Es braucht Python (kommt mit **Entwicklerwerkzeuge**).
3. **Prüfen** zeigt genau, was sich ändern würde, bevor etwas passiert. Bestätigen
   Sie, dann lädt der Server etwa 40 MB herunter.
4. Zum Entfernen **Entfernen** in derselben Zeile nutzen. Das löscht den Zusatz,
   seinen Dienst und seinen Schlüssel. Python und Ihre anderen Werkzeuge bleiben.

Erneutes Installieren ist ungefährlich und repariert eine kaputte Installation.
Das Werkzeug erscheint für Bots, sobald Praxi auf dem Server das nächste Mal
startet (nach einem Neustart oder Update).

**Es gibt keinen Schalter pro Bot.** Ist der Zusatz auf einem Server installiert,
bekommt jeder Bot auf diesem Server `web_scrape`, wie die anderen Web-Werkzeuge.
Die üblichen Freigaben gelten weiter: Ein Bot, der eine Seite gelesen hat, kann
ohne Ihre Nachfrage keine privaten Daten senden. Zum Abschalten für alle Bots
verwenden Sie **Entfernen**.

## Was der Zusatz tut und nicht tut

- Lauscht nur auf `127.0.0.1` (Port 47811); andere Programme brauchen den Schlüssel
  aus `/etc/praxi/scrape.token`.
- Liest nur öffentliche Seiten. Private, lokale und Cloud-Metadaten-Adressen werden
  abgelehnt, jede Weiterleitung wird erneut geprüft; höchstens 5 Weiterleitungen,
  5 MB und 20 Sekunden pro Seite. Das Zeitlimit ist hart: Eine Seite, die ihre
  Daten nur tröpfchenweise sendet, wird am Limit abgebrochen und kann den Zusatz
  nie für die nächste Anfrage blockieren. Nur HTML, XML und Text, keine Cookies.
- Beachtet `robots.txt` samt `Crawl-delay` und nennt sich ehrlich
  `PraxiBot/1 (+https://github.com/emreocell/Praxi-Bot)`.
- Ist höflich: höchstens 1 Anfrage pro Sekunde und 30 pro Minute an eine Website
  (die `robots.txt`-Anfrage zählt mit).
- Die DNS-Adressen des Servers werden aus `/etc/resolv.conf` **bei der Installation**
  in eine kleine Zusatzdatei übernommen. Ändern sich die DNS-Einstellungen später
  (z. B. neuer DHCP-Lease), installieren Sie den Zusatz über **Werkzeuge** erneut.
- Gelesener Text ist **nicht vertrauenswürdiger Seiteninhalt**, genau wie bei
  `web_extract`: Daten zum Lesen, nie Anweisungen.

## Lizenzen

Praxis eigenes `praxi_scrape.py`: Apache-2.0. [Trafilatura](https://github.com/adbar/trafilatura)
2.4.0: Apache-2.0. Abhängigkeiten: Apache-2.0, BSD, MIT (`certifi`: MPL-2.0,
unverändert). Unter Einstellungen → Computer sehen Sie für jedes Paket Lizenz und
Quell-Link; Einzelheiten stehen in `THIRD_PARTY_NOTICES.md`. Eigenständige
Copyleft-Programme (z. B. AGPL-3.0) werden nie mitgeliefert; würde später eines
angeboten, laden Sie es vom Autor und müssen den Lizenzhinweis vorher bestätigen.

## Personenbezogene Daten

Seiten können personenbezogene Daten enthalten. Nach DSGVO/KVKK sind Sie dafür
verantwortlich, was Sie sammeln, warum und wie lange. Lesen Sie nur, was Sie
brauchen, und prüfen Sie die Nutzungsbedingungen einer Website. Das ist keine
Rechtsberatung.
