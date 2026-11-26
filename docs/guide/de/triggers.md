# Wenn etwas passiert

Öffnen Sie im Bot-Panel **Automatisierung → Wenn etwas passiert**. Fügen Sie
eine Regel für neue E-Mails, signierte Aufrufe oder einen Zeitplan hinzu.
Schreiben Sie einen Auftrag, etwa „Bereite eine Antwort zur Prüfung vor“.
Speichern und prüfen Sie die Regel, bevor Sie sie einschalten. Neue Regeln
sind immer zunächst ausgeschaltet.

Bestehende Mail- und Auftragszeitpläne sowie signierte Adressen erscheinen in derselben Liste. Öffnen Sie einen Eintrag, um Ruhezeiten und Grenzen hinzuzufügen. Speichern pausiert ihn bis zum erneuten Einschalten. Erweiterte Workflow-, Vorprüfungs- und Dateiüberwachungszeitpläne bleiben unter Zeitpläne.

Verbinden Sie für E-Mails zuerst ein IMAP-Postfach und erlauben Sie dem Bot
geplante Postfachabfragen. Wählen Sie Ordner, Absender (genaue Adresse,
enthält oder Domain) und optional Betreff oder Anhang. Standardmäßig wird
alle zwei Minuten geprüft, frühestens jede Minute. Die erste Prüfung setzt
den Ausgangspunkt; alte E-Mails starten keine Ausführung. Mehrere neue
E-Mails werden zu einer Liste von Kopfzeilen zusammengefasst. Die Suche
erfasst die neuesten 100 Treffer und garantiert keine Zustellung. AgentMail
liefert derzeit nicht alle erforderlichen Kopfzeilen zur Schleifenvermeidung;
der Editor bietet deshalb nur IMAP-Postfächer an. Anhangfilter benötigen
MIME-Strukturmetadaten vom Server; unbekannte Angaben gelten nicht als Treffer.

Nachrichten von der eigenen Postfachadresse, automatische Antworten,
Massen-/Listenpost und Nachrichten mit Abmelde-Kopfzeile werden übersprungen.
No-reply-Adressen werden nur berücksichtigt, wenn der Absenderfilter sie
ausdrücklich nennt. Ein Absenderfilter authentifiziert keine Absender. Der
E-Mail-Gesprächskanal ist unabhängig: Bei gleichzeitiger Aktivierung können
sowohl ein Gespräch als auch eine Regelausführung entstehen.

Erstellen Sie für Aufrufe unter **Signierte Adressen** eine Adresse und
verknüpfen Sie sie mit der Regel. Signaturprüfung, Wiederholungsschutz und
Aufrufgrenzen bleiben bestehen. Verknüpfte Aufrufe landen in dieser Regel
und verwenden deren Auftrag. Eine ausgeschaltete Regel weist Aufrufe zurück;
der ursprüngliche Webhook-Auftrag wird erst nach Entfernen der Regel wieder aktiv.

Ruhezeiten folgen der gewählten Zeitzone einschließlich Sommerzeitwechseln.
Ereignisse können übersprungen oder für eine gemeinsame Zusammenfassung
danach aufgehoben werden. Standardgrenzen: 10 Ausführungen je gleitender
Stunde und 50 je 24 Stunden pro Regel; 30 und 150 über alle Regeln eines Bots.
Die niedrigste konfigurierte Bot-Grenze gilt für alle seine Regeln. Weitere
Ereignisse warten. Warteschlangen und Zähler überstehen einen Neustart.
Die Anzeige „Heute“ verwendet UTC.

Eine Regel speichert höchstens 100 Ereignisse und 48 KB. Eine volle
Mail-Warteschlange oder eine fehlgeschlagene Ausführung pausiert die Regel.
Auch zehn aufeinanderfolgende Postfachfehler führen zur Pause. Prüfen Sie
Aktivität, Verbindung und Filter vor dem erneuten Einschalten. Bearbeiten
verwirft alte wartende Ereignisse und setzt die Mail-Ausgangsbasis zurück;
Ratenzähler bleiben erhalten. Ein Absturz nach Reservierung einer Gruppe
kann diese verlieren. Es gibt keine Genau-einmal-Zustellgarantie. Bewahren
Sie wichtige Nachrichten im Postfach auf.

**Mit einem Beispiel testen** zeigt den genauen Auftrag und geschützten
Ereignistext mit derselben Größenbegrenzung. Dabei wird kein Modell aufgerufen
und kein Werkzeug ausgeführt. Mail-Regeln lesen nur Kopfzeilen, keine Inhalte
oder Anhänge. Aufruftext wird auf 4.096 Zeichen begrenzt, der Ereignisblock
auf 12 KB. Unsichtbare Zeichen werden aus Kopfzeilen entfernt. Ereignistext
bleibt nicht vertrauenswürdige Daten, auch wenn er zum Ignorieren Ihres
Auftrags auffordert.

Nur der lokale Eigentümer kann Regeln erstellen, bearbeiten oder einschalten.
Der Bot verwendet seine eigenen Berechtigungen und die normalen Genehmigungen;
externe Ausführungen behalten zusätzlich die einmalige Genehmigungspflicht
des Schedulers. Genehmigungspflichtige Aktionen warten und benachrichtigen Sie.
Nichts wird automatisch genehmigt. Zugangsdaten bleiben in der bestehenden
Postfach- oder Webhook-Konfiguration und gehören niemals in die Regel.
