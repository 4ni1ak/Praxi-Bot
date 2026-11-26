# Ortak becerileri yerel uygulamadan yönetme

Yerel sahip, daemon API'siyle ortak becerileri oluşturabilir, içe aktarabilir, güncelleyebilir ve
silebilir. Eşleştirilmiş masaüstleri dahil tüm eşleştirilmiş roller bu değişikliklerden
reddedilir. Olağan loopback bearer ve Origin/Host denetimleri yine geçerlidir.

## Yollar

| Yöntem ve yol | Girdi | Sonuç |
| --- | --- | --- |
| `POST /v1/skills` | `{name, description, instructions}` ya da `{skill_md}` | 201, ad, açıklama, sürüm ve kapsamla |
| `POST /v1/skills/import` | `{kind:"folder", path:"mutlak yol"}` ya da `{kind:"zip", base64:"..."}` | 201, meta verilerle. Mantıksal ad varsa 409 |
| `PUT /v1/skills/{name}` | Aynı belge girdisi, isteğe bağlı `expected_version` | 200. SKILL.md atomik olarak değiştirilir, yardımcı dosyalar korunur |
| `DELETE /v1/skills/{name}?force=false` | İsteğe bağlı açık `force=true` | 200, `deleted` ve `used_by` ile. Atanmışsa etkilenen botlarla 409 |
| `GET /v1/skills` | Mevcut katalog isteği | Yalnızca yerel `locations.shared`, `locations.bot_template`, `locations.bundled` ekler |

`skill_md`, olağan ön bilgi dahil eksiksiz bir SKILL.md dosyasıdır. Alan girdisi aynı biçimi üretir ve
aynı çalışma anı ayrıştırıcısından geçer. Belgenin adı, güncellemede URL ile eşleşmelidir. Ayrılmış
Windows aygıt adları ve `import` yol adı, sahibin yönettiği beceri adları olarak kullanılamaz.

Tüm becerilere yapılan bir atama da kullanım sayılır. Zorla silme, seçili atama adlarını `missing`
olarak yerinde bırakır. Aynı adı yeniden kurmak seçimi geri getirir. Gömülü beceriler ve bot başına
beceriler bu yollarla silinmez. Katalog değişiklikleri ve atama yazımları daemon içinde sıraya
konur. Bağımsız beceri deposu örnekleri ayrıca mevcut dosya sistemi kapsam kilidini paylaşır. Aynı
adla eşzamanlı bir oluşturma, bir başarı ve bir çakışma ile sonuçlanır.

Yayımlanmış `name@version` klasörleri, sahibin güncellemesi boyunca görünür kalır. Belge, klasörü
yeniden adlandırmadan önce atomik olarak değiştirilir. Yalnızca sürüm içeren küçük bir işaret, bir
sonraki denetlenen sahip değişikliğinin ya da bot yayımlamasının yarım kalan yeniden adlandırmayı
bitirmesini sağlar. Yeni çalışma anı okuyucuları, onarım yazımı yapmadan eksiksiz eski ya da yeni
belgeyi kullanabilir. Bu, süreç kesintisini kapsar, dosya sistemi meta verileri için güç kaybı
dayanıklılığı garantilerini kapsamaz.

## İçe aktarma sınırları ve davranışı

- Arşivin/klasörün kökünde tek bir SKILL.md. Sarmalayıcı klasör tahmini yapılmaz.
- En fazla 128 dosya, klasörler dahil 256 girdi, 16 yol parçası, yardımcı dosya başına 1 MiB,
  SKILL.md için 64 KiB ve toplam 8 MiB dosya baytı.
- ZIP yüklemeleri sıkıştırılmış 8 MiB ve 12 MiB JSON istek boyutuyla sınırlıdır.
- Seçilen zip özellikleri yalnızca saklanmış/deflate ZIP girdilerini destekler.
- Her yol parçası ASCII harf, rakam, nokta, alt çizgi ya da kısa çizgidir. Mutlak yollar, üst klasöre
  çıkma, ters eğik çizgiler, sürücü/akış ayırıcıları, gizli girdiler, sondaki noktalar ve Windows
  aygıt adları reddedilir.
- Sembolik bağlar, birleşim noktaları/yeniden ayrıştırma noktaları, özel dosyalar, büyük/küçük harfe
  duyarsız yinelenen yollar ve yürütülebilir Unix dosya kipleri reddedilir. Klasör gezinme bitlerine
  izin verilir. Kaynak kipleri yeni dosyalara asla kopyalanmaz.
- UNIX klasör okumaları, `NOFOLLOW` ile tanımlayıcıya göreli `openat` kullanır. Windows, okurken
  yazma/silme paylaşımı olmadan yeniden ayrıştırma noktası olmayan klasör/dosya tutamaçlarını tutar.
  Değiştirilmekte olan kaynak klasörler/dosyalar reddedilebilir.
- İçe aktarılan hiçbir program, betik, paket kancası ya da komut çalıştırılmaz. Olağan, yürütülemez
  kiplere sahip yardımcı betik kaynak dosyaları, etkisiz veri olarak saklanabilir.
- Bilinen gizli bilgiler, yalnızca ortam değişkeniyle kayıtlı değerler dahil, çalışan ajanın üretim
  maskeleyicisiyle, herhangi bir yayımlamadan önce denetlenir. Eşleşen bir belge ya da yardımcı dosya
  sessizce yeniden yazılmaz, reddedilir.

Sahibin oluşturduğu dosyalar mevcut ortak/kullanıcı kaynağını kullanır. Bu, botun `skill_write` ya da
`skill_publish` davranışını değiştirmez. `11917d2` sürümündeki ana dalda henüz yerel bir #89
güven yan dosyası/bulaşma uygulaması yoktur. Bu PR, o sistemi getirdiğini iddia etmez. #89
uzlaştırılırken bot tarafındaki işaretlemelerini koru.

Her değişiklik, beceri dosyalarını değiştirmeden önce ana makine tarafından yazılmış bir denetim
niyeti kaydeder. Yük yalnızca dosya sayısını içerir, adları, yönergeleri, kaynak yollarını ya da
arşiv içeriğini asla içermez. Denetim hatası değişikliği engeller. Yanıtlar ve katalog meta verileri
üretim maskeleyicisini kullanır. Ayrıştırıcı ve dosya sistemi hataları, verilen içeriği yansıtmadan
sabit mesajlar döndürür.

Mevcut seçici yenilemesi bu yolları doğrudan kullanabilir. Burada yeni bir Tauri komutu, arayüz
penceresi, CSS ya da yerelleştirilmiş metin eklenmez.

## Bir git deposundan kurma (önce önizle, sonra kur)

`POST /v1/skills/hub/preview`, `{ "url": "https://host/owner/repo.git", "rev": null, "subdir": null }`
gövdesiyle depoyu özel bir hazırlık klasörüne çeker, tarar ve becerinin adını, bir risk düzeyini
(`low`, `medium`, `high`), bulguları (betikler, ağ erişimi, kimlik bilgisi desenleri, gizleme,
yönerge enjeksiyonu) ve bir `fingerprint` döndürür. Hiçbir şey kurulmaz.

`POST /v1/skills/hub/install` aynı gövdeyi, artı `"accept": "<fingerprint>"` alanını alır. Depoyu yeniden
çeker ve yalnızca yeni parmak izi kabul edilene eşitse kurar (aksi halde `409 skill_changed`). Kurulum,
`/v1/skills/import` ile aynı sahip yolunu kullanır: bağlar ve yürütülebilir dosyalar reddedilir, gizli
bilgiler reddedilir, hiçbir şey çalıştırılmaz. Yalnızca yerel sahip. Yalnızca adlandırılmış bir ana
makinedeki `https://` depoları.
