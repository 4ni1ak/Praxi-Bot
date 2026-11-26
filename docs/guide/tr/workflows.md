# İş akışları: kaydet, yeniden oynat, hatırla

Her tıklama için bir dil modelini çağırmak yavaştır ve para harcatır. Praxi'nin cevabı, bir kez işe yarayan bir görevi **model çağrısı olmadan** yeniden yapabileceği bir şeye çevirmektir. Birbiriyle ilişkili üç mekanizma var. Bugün hangilerinin var olduğunu bil:

| Mekanizma | Ne yapar | Durum |
| --- | --- | --- |
| **Yeniden oynatma** | Kaydedilmiş, parametreli bir iş akışını belirlenimci biçimde çalıştırır | Kullanılabilir |
| **Hatırlama** | Praxi'ye “bu istek doğrudan az önce işe yarayan yola gitsin” dersin | Kullanılabilir (masaüstü uygulamasındaki düğme, API) |
| **Kaydetme** | Başarılı bir çalışmayı gözden geçirir ve değişiklik yapan adımlarını açıkça kaydedersin | Kullanılabilir (masaüstünde gözden geçirme, API, CLI) |

## İş akışları

Bir iş akışı tek bir bota aittir ve `<data dir>/bots/<bot-id>/workflows/<name>.toml` yolunda saklanır. Dizini kendiliğinden yeniden oluşturulur.
`<data dir>/workflows` altındaki eski dosyalar bulunmaz, çünkü bir iş akışı tam olarak tek bir bota ait olmalıdır. Eski birini korumak için onu gözden geçir ve dosyayı o botun `workflows/` klasörüne elle kopyala.
Bu konum, bot dosya araçlarından korunan daemon durum sınırının içindedir.
Bir iş akışının **parametreleri** ve bir **adım** listesi vardır. `workflow_list` ve `workflow_run`, çağıran botun deposunu kullanır.

```toml
version = 1
name = "notepad_note"
description = "Open Notepad and type a note"

[[params]]
name = "text"
type = "string"          # string | number | bool
default = "hello"        # varsayılan yoksa zorunludur
description = "Text to type"

[[steps]]
kind = "launch"
executable = "notepad.exe"

[[steps]]
kind = "desktop_action"
app = "notepad.exe"
window_title_pattern = "*Notepad"   # * ve ? joker karakterleri, büyük/küçük harf duyarsız
action = "type"                     # click | type | set_value | key
value = "{text}"
[steps.target]
role = "edit"
name = "Text editor"
fingerprint = "9f2c0a41d3b7e855"    # kaydedilen kimlik (onaltılık)
```

Adlarda harf, rakam ve basit ayırıcılar kullanılır. `{param}` yer tutucuları, iş akışı çalışırken doldurulur.

### Bir çalışmayı kaydet ve gözden geçir

Başarılı bir üst düzey çalışmadan sonra bir öneri istemek için **İş akışı olarak kaydet** düğmesine bas. Tüm adımları gözden geçirip açıkça kaydetmedikçe hiçbir şey kaydedilmez. Okuma ve meta çağrıları listelenir ama dışarıda bırakılır. Desteklenmeyen ya da eksik değişiklik çağrıları kaydı uygunsuz yapar: kısmi otomasyon kaydedilemez. Normal model yoluyla yapılan çalışmalar da uygundur.

Öneri, parametrelerin kökenini ve önceki dış ya da özel araç çıktısını izler. Bilinmeyen kökenler temkinli biçimde ele alınır. Etkilenen her adımı gözden geçir ve onayla. Kaydedilen dosyalar kaynak çalışmayı, botu ve kaynak içerik işaretini korur. Yeniden oynatma bu işareti devralır ve kalıcı bir izin olsa bile yeniden onay ister.

Kaydedilen veri alanları, yani hedefler, yollar, komutlar, kimlik bilgileri ve yazılan tüm metin, varsayılansız zorunlu parametrelere dönüşür. Bu bilerek yapılır: bir alıcıyı ya da hedefi sessizce yeniden kullanmak yerine her seferinde yeni girdi istenir. Böyle iş akışları bulanık yönlendirmeyle ya da modelin `workflow_run` aracıyla seçilemez. Parametre değerlerini sohbet geçmişinin, çalışma adımı girdilerinin, olay günlüklerinin ve sağlayıcı isteklerinin dışında tutan insan yeniden oynatma uç noktasını ya da CLI'ı kullan:

```text
bot workflow propose <run-id>
bot workflow save reviewed-proposal.json
bot workflow list --bot <bot-id>
bot workflow show <name> --bot <bot-id>
bot workflow run <name> --chat <chat-id> --params-stdin
bot workflow delete <name> --bot <bot-id>
```

Onay gerektiren bir öneride, CLI ile kaydetmeden önce üst düzeydeki `confirmed_steps` dizisini, gözden geçirdiğin adımların sıfırdan başlayan numaralarını içerecek şekilde düzenle. `--params-stdin`, parametre adlarından ve metin değerlerinden oluşan bir JSON nesnesini standart girdiden okur. Bunu özel bir girdi akışıyla ver, kimlik bilgilerini hiçbir zaman kabuk argümanlarına yazma. Çalışma zamanı değerleri şu an en az altı bayt olmalıdır, çünkü ortak maskeleyici daha kısa değerleri bilerek yok sayar. Desteklenmeyen kısa değerler reddedilir. Tırnak, satır sonu ve ters eğik çizgi içeren değerler kabul edilir. Değerler yalnızca işlem belleğinde kalır: yeniden başlatmadan sonra yeni bir yeniden oynatma başlat ve değerleri yeniden ver. Yarıda kalan bir insan yeniden oynatması, modele devir olmadan başarısız olur.

API, iş akışı listeleme, gösterme ve silme isteklerinde `bot_id` ister, kaydetme kapsamını kaynak çalışmadan, yeniden oynatma kapsamını ise sahip olunan `chat_id` değerinden çıkarır. Telefon rolündeki eşlerin iş akışı okuma, öneri, değiştirme ya da yeniden oynatma erişimi yoktur.

### Adım türleri

| `kind` | Ne yapar |
| --- | --- |
| `launch` | Bir masaüstü uygulaması başlatır |
| `desktop_action` | Bir penceredeki bulunmuş bir öğeye tıklar, yazar, değer atar ya da tuş kombinasyonuna basar |
| `browser_action` | Bulunmuş bir sayfa öğesine tıklar, doldurur, seçer, bir tuşa basar ya da geri gider |
| `navigate` | Tarayıcıda bir URL açar |
| `wait_for` | Bir pencere başlığını, sayfa metnini, bir CSS seçicisini ya da ağın boşalmasını bekler (varsayılan 15 sn) |
| `shell` | Bir komut çalıştırır (`shell_run` üzerinden) |
| `tool` | Şablonlu bir girdiyle kayıtlı herhangi bir aracı çağırır |
| `agent` | Açıkça “modele sor” diyen bir alt görev |

Öğeler, güvenilirlik sırasına göre birkaç yöntemle bulunur (rol ve ad, erişilebilirlik etiketi, CSS seçicisi, konum ipuçları) ve kaydedilmiş bir parmak iziyle karşılaştırılır, böylece adı değişmiş bir düğme fark edilir.

### Bir iş akışını çalıştırma

- Bota söyle: “notepad_note iş akışını text değeri “saat 3'te toplantı” olacak şekilde çalıştır”. Bot `workflow_run` aracını `{"name": "notepad_note", "params": {"text": "..."}}` ile çağırır.
- Eğik çizgi komutunu yaz: `/workflow notepad_note text="meeting at 3"`.
- Bir [zamanlamaya](scheduled-tasks.md) bağla (API ya da masaüstü uygulaması üzerinden).

Ekran hâlâ kayıtla örtüştüğü sürece yeniden oynatma model gerektirmez. Örtüşmediği anda (bir öğenin adı değişmiş ya da belirsiz, bir eylem başarısız, bir izin reddedilmiş, bir `agent` adımı gelmiş, bir başarı denetimi tutmamış) yeniden oynatma **durur ve işi** bota **devreder**, hangi adıma geldiğini bildirir, böylece bot oradan devam edebilir. Her adım yine de doğrudan bir araç çağrısı gibi izin sisteminden geçer: bir iş akışı, botunun yapamayacağı hiçbir şeyi yapamaz. `workflow_run` aracının kendisi `script` kategorisindedir.

## Hatırlama

Bir çalışma **başarılı** olduktan sonra onu hatırlamaya değer diye işaretleyebilirsin. Masaüstü uygulamasında yanıtın altındaki **Bunu hatırla** düğmesine bas. API üzerinden bu `POST /v1/runs/{id}/remember` çağrısıdır. Bundan sonra aynı istek metni, ana modelden karar vermesini istemeden, işe yarayan yola (parametreleriyle kaydedilmiş bir iş akışına, ya da bir araç veya beceri ipucuna) doğrudan yönlendirilir.

Neyi hatırlayabilir:

- yönlendiricinin o istek için seçtiği yolu, ya da
- modelin `workflow_run` ile çalıştırdığı kayıtlı iş akışını.

Neyi reddeder: başarısız çalışmaları, alt ajan çalışmalarını, hiçbir yolu izlemeyen ve hiçbir iş akışı çalıştırmayan çalışmaları ve yönlendiricisi kapalı botları. Bir iş akışını düzenlersen ya da silersen, onun için hatırlanan yollar silinir. Hatırlanan yolların süresi bir süre sonra dolar.

## Karar merdiveni, kısaca

Ana model bir iletiyi görmeden önce Praxi daha ucuz basamakları sırayla dener, her biri başarısız olursa ana modele geçer:

| Basamak | Mekanizma |
| --- | --- |
| K0 | Eğik çizgi komutları ve kendi regex kuralların |
| K1 | Yukarıdaki hatırlanan yollar |
| K2 | Anahtar sözcük (BM25) ve bir Gemini anahtarı ayarlıysa, iş akışların, becerilerin ve araçların üzerinde gömme araması |
| K3 | Kesin JSON ile yanıt veren hızlı bir sınıflandırıcı model (`models.classifier`) |
| K4 | Tam döngüyle ana model |

Bir iş akışı yalnızca yüksek güvende (0,9) ve zorunlu parametrelerinin hepsi biliniyorsa kendiliğinden seçilir. Aksi halde sonuç, modelin gördüğü araçları daraltan bir ipucu olur. CLI'da karar `[route ...]` olarak yazdırılır. Tüm merdiveni bot başına `[router] enabled = false` ile kapatabilirsin.

## Planlanan

- Masaüstü uygulamasında bir iş akışı düzenleyicisi.
