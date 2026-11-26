# İsteğe bağlı karar havuzu

Karar havuzu, bir kullanıcı mesajı hakkında küçük, yapılandırılmış sorular sormak için isteğe bağlı bir
olanaktır. Mevcut K0 ile K3 arasındaki yönlendirici ve [model katmanlama](model-tiering.md), olağan
turların nasıl çalışacağını yine seçer. Bu sürüm havuzu hazırlar ama olağan turlardan çağırmaz.

## Gemini için hızlı anahtar

Yalnızca Gemini yardımcısını istiyorsan **Ayarlar > Modeller > Akıllı arama ve yardımcı > Karar
yardımcısı** bölümünü aç ve onu aç. Bir Gemini anahtarı gerekir ve dakikada 180 istekle
(ücretsiz katmanın 200 isteğinin yüzde 90'ı. Ücretsiz katman ayrıca dakikada 50.000 token ve günde
10.000 isteğe izin verir, son kontrol 2026-11-24) `gemini/gemini-decision-1` için tek bir
`gemini_decisions` arka ucu olan bir `decider.toml` yazar. Ayarlar, senin yazdığın bir `decider.toml`
dosyasının üzerine asla yazmaz. Bkz. [Yardımcılar (isteğe bağlı)](embeddings.md).

## Havuzu yapılandır

`decider.toml` dosyasını daemonun veri klasöründe, `providers.toml` dosyasının yanında oluştur. Model
başvurularını, etkin sağlayıcılarında bulunan modellerle değiştir.

```toml
enabled = true
timeout_ms = 1500
max_calls_per_turn = 2

[[backends]]
api = "gemini_decisions"
model = "gemini/your-decision-model"
rpm = 20

[[backends]]
api = "chat"
model = "provider/your-small-json-chat-model"
rpm = 10
```

| Ayar | Anlamı |
| --- | --- |
| `enabled` | Varsayılanı `false`'tur. Açıldığında öncelik sırasıyla bir ile sekiz arası arka uç ver. |
| `timeout_ms` | Tüm zincir için tek bir süre sınırı. Varsayılanı 1.500'dür, 1 ile 1.500 arası kabul eder. |
| `max_calls_per_turn` | Varsayılanı 2'dir, 1 ile 8 arası kabul eder. Hatalar ve zaman aşımları dahil her deneme bir çağrı tüketir. |
| `backends[].api` | `chat` (varsayılan), seçilen sağlayıcının JSON sohbet kipini kullanır. `gemini_decisions` ve `openrouter_decisions`, ilgili yerleşik sağlayıcıyı ve onun yerel Decisions uç noktasını kullanır. |
| `backends[].model` | Gerekli, tam model başvurusu. OpenRouter için `openrouter/vendor/model` gibi üst akış üreticisini de yaz. Bir model zincirde yalnızca bir kez geçebilir. |
| `backends[].rpm` | Bu arka uç için gerekli, kayan dakikalık istek sınırı, 1 ile 10.000 arası. Yerel bir HTTP 429 o arka ucu bir dakika duraklatır. |

Küçük ve JSON destekleyen bir sohbet modelini [sağlayıcı kataloğundan ya da sağlayıcının model
listesinden](providers-and-models.md) seç. Yerel Decisions için [Gemini Decisions](https://aistudio.google.com/docs/decisions-api)
ya da [OpenRouter Decisions kataloğunda](https://openrouter.ai/models?categories=decisions) listelenen
bir model kullan. Depodaki derlenmiş katalog sohbet uç noktalarını tanımlar. OpenRouter'ın
[yerel Decisions API'si](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-request)
alfa aşamasındadır ve ayrı bir model listesi vardır. Havuz varsayılan bir model kimliği vermez ve
ücretsiz modelleri kendiliğinden keşfetmez. Ayarladığın model için sağlayıcının fiyatını, kotasını ve veri
işleme koşullarını, ücretsiz bir katmanın sağlayıcının istemleri eğitim için kullanmasına izin
verip vermediği dahil, kontrol et.

Kimlik bilgilerini [olağan sağlayıcı kurulumuyla](providers-and-models.md) ayarla. Havuz, ayarlanmış
sağlayıcının gizli bilgi başvurusunu ya da katalogdaki ortam değişkenini kullanır. Değişiklikleri
uygulamak için daemonu yeniden başlat ya da sağlayıcıları yeniden yükle. RPM pencereleri bellektedir ve
sağlayıcı yapılandırması yeniden yüklendiğinde sıfırlanır. Kullanılamayan bir arka uç atlanır. Geçersiz
yapılandırma havuzu devre dışı bırakır. `bot doctor` ayarları, modelleri yoklamadan bildirir.

## Girdi, yanıtlar ve hesaplama

Ana makine yalnızca özgün, üst düzey insan mesajını ve güvenilir soru meta verisini sağlar. İkisini de
göndermeden önce canlı maskeleyiciden geçirir. Geçmiş, ekler ve araç sonuçları isteğin dışında kalır.
Bir soru, 0 ile 1 arasına sıkıştırılmış bir olasılık döndürebilir ya da sunulan 2 ile 255 arasındaki
kimliklerden birini seçebilir. Serbest metin, bilinmeyen kimlikler ve retler atılır. Yanıtların araçlar,
izinler, onaylar ya da araç bağımsız değişkenleri üzerinde hiçbir yetkisi yoktur. Yanıtlanmayan sorular,
çağıranın belirlenimci yolunu kullanılabilir bırakır.

Ana makine bağdaştırıcısı, gönderilen her denemeyi [LLM çağrı defterinde](llm-call-ledger.md)
`decider` altında kaydeder. Bildirilen token sayıları varsa kullanılır. Defter girdisinin
başlatılamaması gönderimi engeller.
