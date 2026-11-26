# Discord

Sprechen Sie mit Ihren Bots über Discord: in einer privaten Nachricht an Ihr
eigenes Bot-Konto oder in einem Server-Kanal, den Sie erlauben. Nur **Ihr**
Discord-Konto kann es nutzen: alle anderen werden ignoriert (in einer privaten
Nachricht erhalten sie eine einzige höfliche Zeile).

## Verbinden (etwa 5 Minuten)

```sh
bot channels discord setup     # erklärt die Schritte, fragt das Token (verdeckt) ab und testet es
bot serve                      # der Kanal startet mit dem Hintergrunddienst
bot channels discord pair      # gibt einen einmaligen 6-stelligen Code aus (10 Minuten gültig)
```

1. Öffnen Sie <https://discord.com/developers/applications> und klicken Sie auf **New Application**.
2. Öffnen Sie **Bot**, klicken Sie auf **Reset Token** und kopieren Sie das Token.
3. Schalten Sie auf derselben Seite unter **Privileged Gateway Intents** den Schalter **Message Content Intent** ein. Ohne ihn schickt Discord dem Bot leere Nachrichten.
4. Führen Sie `bot channels discord setup` aus und fügen Sie das Token ein. Es landet in Ihrem Geheimnisspeicher und wird nie wieder angezeigt.
5. Das Setup endet mit einem Verbindungstest und einem Einladungslink. Fügen Sie den Bot damit einem eigenen Server hinzu. Öffnen Sie dann sein Profil in der Mitgliederliste und wählen Sie **Nachricht**.
6. Öffnen Sie in Discord eine **private Nachricht** an Ihren Bot und senden Sie `!pair 123456` (Ihr Code).

Für deutsche Einrichtungsschritte und Antworten verwenden Sie `bot channels discord setup --lang de`. Die Sprache der Antworten können Sie später mit `!lang` ändern.

`bot channels discord test` wiederholt den Verbindungstest jederzeit.

## Wer mit dem Bot sprechen darf

- Die Kopplung funktioniert nur in einer privaten Nachricht an den Bot und nur mit einem Code, den Sie an Ihrem Computer erzeugt haben. Ein Code in einem Server bewirkt nichts; erzeugen Sie einen neuen.
- Das gekoppelte Konto ist der **Besitzer**. Niemand sonst wird bedient, auch nicht in einem erlaubten Server-Kanal. Ein neuer gültiger Code von einem anderen Konto ersetzt den Besitzer und vergisst die erlaubten Server-Kanäle.
- Bots und andere Apps werden ignoriert.
- Falsche Codes werden abgelehnt und nach einigen Versuchen für eine Weile gesperrt.

## Im Chat

Befehle beginnen mit `!` (ein `/` funktioniert auch, wenn es als normaler Text gesendet wird).

| Nachricht | Wirkung |
| --- | --- |
| `!bots` | Listet Ihre Bots auf |
| `!use <Name>` | Wählt den Bot für diesen Chat |
| `!stop` | Stoppt die laufende Aufgabe |
| `!status` | Zeigt, wie viele Aufgaben laufen und auf Ihr OK warten |
| `!lang en`, `!lang tr`, `!lang de` | Sprache der eigenen Nachrichten des Bots |
| `!allow` / `!disallow` | Im Server-Kanal: dem Bot dort Antworten an Sie erlauben / beenden (nur Besitzer) |
| `!help` | Kurze Hilfe |
| alles andere | Geht an den gewählten Bot |

In einem erlaubten Server-Kanal geht **jede** Ihrer Nachrichten an den Bot. Wählen
Sie daher einen Kanal nur dafür.

## Freigaben

Braucht ein Bot Ihr OK, erhalten Sie eine Nachricht mit den Schaltflächen
**Einmal erlauben** und **Ablehnen**. Nur Sie können sie drücken, nur in dem Chat,
in dem die Karte erschien, einmal und innerhalb von 10 Minuten.

- Schaltflächen gibt es nur für Anfragen mit geringem Risiko, deren voller Text in die Nachricht passt. Alles andere (riskante Aktionen, Anfragen mit verborgenen Geheimnissen, sehr lange Anfragen) sagt das und wartet auf die App an Ihrem Computer.
- „Immer erlauben“ gibt es hier nicht. Dauerhafte Erlaubnisse werden in der App entschieden.

## Nachrichten und Anhänge

- Lange Antworten werden auf mehrere Nachrichten verteilt und etwa einmal pro Sekunde aktualisiert. Das letzte unvollständige Wort bleibt verborgen, bis Geheimnisse sicher ausgeblendet werden können.
- Der Bot pingt niemanden: `@everyone`, `@here` sowie Nutzer- und Rollenerwähnungen in einer Antwort werden unwirksam gemacht.
- Anhänge werden **nicht heruntergeladen oder geöffnet**. Der Bot erfährt nur, dass eine Datei mit einem bestimmten Namen und Typ gesendet wurde, als nicht vertrauenswürdig markiert. Senden Sie stattdessen den Text.
- Alles, was an Discord geht, läuft durch den Geheimnisfilter, der bekannte Geheimnisse verbirgt, auch das Bot-Token.
- Der Bot hält Discords Ratenlimits ein und wartet, wenn Discord darum bittet.

## Gut zu wissen

- Sie erstellen Ihre eigene Discord-Anwendung, das Token gehört Ihnen. Der Bot verbindet sich von Ihrem Computer nach außen; ein offener Port ist nicht nötig.
- Discord-Nachrichten laufen über die Server von Discord. Nutzen Sie es nicht für etwas, das Sie nicht über Discord senden würden.
- `bot channels discord status` zeigt, ob der Kanal eingerichtet, aktiv und laufend ist und ob ein Besitzer gekoppelt ist. `bot channels discord disable` schaltet ihn beim nächsten `bot serve` ab; das Token bleibt gespeichert.
- Sagt der Bot nichts: Prüfen Sie mit `bot channels discord test` Token und Message Content Intent und ob `bot serve` nach dem Setup neu gestartet wurde.

- Wer den Server-Kanal lesen kann, sieht auch die Antworten des Bots. Wählen Sie einen privaten Kanal.
- Lassen Sie `bot serve` laufen. Nachrichten aus der Zeit, in der der Dienst ausgeschaltet war, werden später nicht abgeholt.
