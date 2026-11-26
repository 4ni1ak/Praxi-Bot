# Notlar

Her botun, çalışmalar arasında kullandığı küçük bir yerleşik not defteri vardır: uzun bir görevin planı, bir kontrol listesi, yarın lazım olacak bilgiler. Araçlar: `notes_read` (notları listeler, birini okur ya da arar) ve `notes_write` (bir notu değiştirir, ekleme yapar ya da siler).

* Notlar tek bir bota aittir, diğer botlar onları hiç görmez. Bilgisayarında kalırlar.
* Notlarda parola ya da anahtar saklanmaz: kimlik bilgisine benzeyen metin reddedilir.
* Sınırlar: not başına 32 KB, en fazla 100 not, bot başına 512 KB.
* Güvenlik: bot bir web sayfasını, bir e-postayı ya da başka dış içeriği okuduktan sonra not kaydetmek veya silmek önce senin onayını ister, çünkü bir not botun sonra ne okuyacağını şekillendirir. Böyle bir çalışmada kaydedilen not güvenilmez olarak hatırlanır ve bota talimat olarak değil, veri olarak gösterilir.

Notlar hafızadan farklıdır: hafıza, botun sana ait olduğunu söylediğin bilgileri tutar. Notlar ise botun kendi çalışma alanıdır.

## Hafıza araçları

Hafızası açık olan bir botun dört aracı vardır: `memory_search` (hatırladıklarına bakar), `session_search` (daha önceki bir sohbette söyleneni bulur), `memory_save` ve `memory_forget`. Hafızası **Kapalı** olan bota bu araçlar verilmez. Bkz. [Hafıza](memory.md).

* `memory_save` yalnızca bu turda söylediğin şeyi saklar (mesajından alıntı yapmak zorundadır), gizli bilgileri, soruları ve alt ajan çalışmalarını reddeder, çalışma dış içerik okuduysa onay ister. Botun hafıza ayarını izler: **Kaydetmeden önce sor** seçiliyken not senin onayını bekler, **Öğrendiklerimi diğer botlarımla paylaş** anahtarı ise senin hakkındaki bilgilerin diğer botlarınla paylaşılıp paylaşılmayacağını ya da bu botta kalıp kalmayacağını belirler.
* Arka planda öğrenme bu araçlardan ayrıdır: bir turdan sonra bot kendiliğinden de not tutabilir. Buna araç kullanan, alt çalışma başlatan ya da dış içerik (web sayfaları, dosyalar, e-posta, diğer botlar) okuyan turlar da dahildir. Böyle turlardan çıkan notlar **güvenilmez** diye etiketlenir, **Onayın gerekiyor** bölümünde bekler ve sen onaylayana kadar kullanılmaz. Modele yalnızca alıntılanmış veri olarak gösterilirler, hiçbir zaman talimat olarak değil.

## Saat ve hesap makinesi

Botun iki küçük yardımcı aracı her zaman vardır, ama yalnızca gerektiğinde yüklenirler (bot onları araçları arayarak bulur):

* `time_now`, herhangi bir saat diliminde güncel tarih ve saati verir ve tarih hesabı yapar: gün, ay ya da saat ekler veya çıkarır, bir tarihe ne kadar kaldığını sayar ya da aynı anı başka dilimlerde gösterir. Böylece bot bugünün tarihini tahmin etmek zorunda kalmaz.
* `calculate` kesin aritmetik yapar (`+ - * / % ^`, parantezler, `round` ve `sqrt` gibi birkaç işlev), böylece toplamlar ve yüzdeler “kafadan” hesaplanmaz.

İkisi de yalnızca botun verdiği metin üzerinde hesap yapar ve saati okur. Dosya açamaz, ağı kullanamaz, program çalıştıramaz. Bu yüzden hiçbir zaman onay istemez ve dışarıya bir şey gönderme sayılmaz. İfadelerin uzunluğu ve derinliği sınırlıdır, düz aritmetik olmayan her şey reddedilir.

Bir CSV, JSON ya da PDF dosyasını okumak (`csv_read`, `json_query`, `pdf_extract`) yalnızca izin verdiğin klasörlerde çalışır ve özel veri okuma sayılır: bot az önce bir web sayfası ya da e-posta okuduysa, veriyi dışarı gönderecek her şey önce sana sorar.
