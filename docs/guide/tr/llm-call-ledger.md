# LLM çağrı defteri

Artık her model çağrısı bir amaç kaydeder. Buna yürütücü istekleri, koruma kapanış istekleri, K3
yönlendirmesi, K2 gömme istekleri, arka plandaki hafıza çıkarma/birleştirme ve alt çalışmalar dahildir.
[Karar havuzu](decision-pool.md), her arka uç denemesi için `decider` amacını kullanır. Olağan turlar
havuzu henüz çağırmaz.

| Amaç | Neyin sayıldığı |
| --- | --- |
| `executor` | Yeniden denemeler ve yedekler dahil ana sohbet modeli çağrıları |
| `guard_close` | Bir döngü koruması tetiklendikten sonraki, araçları kapalı kapanış çağrısı |
| `router` | K3 sınıflandırıcısı |
| `embedding` | Her K2 sorgusu ya da aday yenileme aktarım partisi |
| `memory` | Her arka plan çıkarma ya da birleştirme çağrısı |
| `subagent` | Bir alt çalışmanın model çağrıları. Üst çalışmaların özetlerine de dahil edilir |
| `decider` | Yedekler dahil tek tek karar havuzu arka uç çağrıları |

## Denetim kayıtları

`llm.call_started`, gönderimden önce kaydedilir. Bu yazım başarısız olursa çağrı gönderilmez. `usage`,
aynı opak `call_id` değerini tamamlar. Sayısal alanları `calls`, `step_n`, `tool_rounds`, `latency_ms`,
`prompt_tokens`, `completion_tokens`, `cached_tokens` ve isteğe bağlı `cost_usd` alanlarıdır. Başarısız/
zaman aşımına uğramış çağrılar ve K2 gömmeleri dahil hiçbir kullanım bildirilmediğinde `tokens` nesnesi
`null` olur. Bilinmeyen token sayıları, sıfır token faturalandırıldığı anlamına gelmez.

Token bildiren ama sonra akışı başarısız olan bir istek, bildirilen sayıları korur. Bir sağlayıcı
çağrısı, birkaç kullanım anlık görüntüsü yayınlasa bile bir kez sayılır ve en son anlık görüntü
geçerlidir. Döngünün yaptığı yeniden denemeler ayrı çağrılardır. Değişmemiş bir sağlayıcı
bağdaştırıcısının içinde gizli kalan aktarım yeniden denemeleri burada ayrıca gözlemlenemez. CLI
sağlayıcıları, kendi iç model çağrılarını değil, tek bir oturum çağrısı gösterir.

Bu satırlar istem, araç girdisi/çıktısı, yanıt metni ya da sağlayıcı hata metni içermez. Model etiketi
ve diğer tüm metinler canlı üretim maskeleyicisinden geçer. Hiçbir gizli bilgi ya da ayrı bir kullanım
veritabanı getirilmez.

Tamamlanınca `run.usage_summary`, amaçtan `{calls, prompt_tokens, completion_tokens, cached_tokens,
cost_usd}` yapısına tek bir eşleme içerir. O çalışmanın bekleyen arka plan hafıza/gömme işi ve
alt çalışmaları bittikten sonra bir kez yazılır. `AgentLoop::wait` yine turu bekler. `flush_background`
ayrıca arka plan hesaplamasını da bekler. Duraklatılmış çalışmaların henüz son bir özeti yoktur. Bir özet
üretmek için hiçbir model çağrısı yapılmaz.

Niyet ve sonuç satırları `call_id` ile eşleşir. Bir çökmeden sonra tamamlanmamış bir niyet, bilinmeyen
tokenlarla sayılabilir kalır. Niyeti kaydetmekle çağrıyı göndermek arasında kaçınılmaz, dar bir boşluk
vardır: tamamlanmamış bir niyet, gönderim denendiğinin kanıtıdır, sağlayıcının faturalandırma makbuzu
değildir.

## API

`GET /v1/usage`, `runs`, `total` ve `by_bot` alanlarını korur ve aynı toplam yapısıyla `by_purpose`
ekler. Toplamlar, bilinmeyen tokenlı çağrıları da içerir. Her çağrı tam olarak bir amaç grubunda
görünür, bu yüzden grupları toplamak toplamları verir. Yalnızca geçerli kullanıcının seçili çalışmaları
dahil edilir. `since_ms` yine çalışma oluşturma zamanına göre süzer. Üst çalışma özetleri genel
toplamlara eklenmez (aksi halde alt çağrılar iki kez sayılırdı).

API, önceki güncel fiyat anlamını korur: bilinen token sayıları, o anda yüklü katalog/yapılandırmaya
göre fiyatlandırılır. Denetim özetleri, çağrı tamamlandığında yakalanan fiyat tahminini korur. Geçmiş
model adımları ve yükseltmeden sonra sürdürülen karışık geçmişler, karşılık gelen bir defter çağrısı
yoksa yine sayılır. Geçmiş amaçlar yalnızca kaydedilmiş adım/çalışma meta verisinden çıkarılabilir.

## Değerlendirmeler

JSONL şema sürüm 2, `llm_calls_by_purpose: {purpose: count}` alanını ekler. Mevcut `llm_calls` alanı
model adımlarını saymaya devam eder ve eski çalışmalarla karşılaştırmaları korur. Amaç eşlemesi, gerçekte
gözlemlenebilir çağrıları sayar ve yeniden denemeleri, yönlendirici/arka plan işini ve alt çalışmaları
içerir. Bu yüzden toplamı `llm_calls` değerini aşabilir. Şema 1 dosyaları yine boş bir amaç eşlemesiyle
çözümlenir. `bot eval report`, kaydedilmiş çalışmalar boyunca ham çağrı toplamlarını içeren ayrı bir
amaç tablosu ekler. Eksik geçmiş dökümleri sıfır olarak değil, bilinmiyor olarak gösterilir.

Veritabanı geçişi, yeni yol, arayüz bileşeni, Tauri komutu, sağlayıcı fiyat değişikliği ya da sağlayıcı
bağdaştırıcısı değişikliği gerekmez.
