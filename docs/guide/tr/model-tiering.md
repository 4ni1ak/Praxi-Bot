# İsteğe bağlı model katmanlama

Model katmanlama, bir botun basit ilk turların küçük bir bölümü için ayarlanmış hafif bir model
kullanmasını sağlar. Botun model yapılandırmasına `light` ekle:

```toml
[models]
executor = "provider/your-main-model"
light = "provider/your-light-model"
```

İki başvuruyu da ayarlanmış sağlayıcılarının sunduğu modellerle değiştir. Hiçbir sağlayıcı ya da fiyat
varsayılmaz. Araçlarını destekleyen bir hafif model seç. İki başvuru da `provider/model` biçiminde
olmalıdır, `cli:` ile başlayan bir hafif model reddedilir. Yalnızca yürütücü kullanan eski seçime
dönmek için `light` satırını sil. Kurulum hazır ayarları katmanlamayı kapalı bırakır.

## İlk model nasıl seçilir

Katman seçimi başka bir model isteği yapmaz. Bu sürüm, mevcut yönlendirme basamağı bittikten sonra
belirlenimci bir kural kullanır: en çok 200 Unicode karakterlik bir kullanıcı mesajı, basamak boş olmayan
bir araç ipucu döndürdüğünde ve ipucu verilen her araç görünür olup `Effect::Read` taşıdığında uygundur.
Rota `Agent` ise ya da yönlendirme kapalıysa, büyük/küçük harf ve çevresindeki boşluklar yok sayılarak
tam olarak `merhaba`, `selam`, `hello` ve `hi` selamlarını da kabul eder. Bir selamın sonuna başka
yönergeler eklemek onu uygun yapmaz.

Diğer istekler yürütücüyü kullanır. Seçim, ilk kullanıcı adımında `tier` olarak saklanır.
`route_decided` aynı isteğe bağlı alanı taşır. `light` olmayan botlar, yeni bir `tier` alanı olmadan
önceki olay ve adım biçimini korur. Yönlendirici mevcut süre bütçesine ulaşırsa belirlenimci kural,
başka bir sınıflandırıcı isteği ya da zaman aşımı olmadan onun yedek rotasına uygulanır.

İsteğe bağlı [karar havuzu](decision-pool.md), ileride yönlendirme birleşimleri için kullanılabilir.
Katman seçici belirlenimci kuralı kullanmaya devam eder.

## Yürütücü ne zaman devralır

Model, şu koşullardan biri oluşana kadar hafif kalır:

- Hafif sağlayıcı desteklenmeyen bir yetenek bildirir. Art arda iki bozuk sağlayıcı yanıtı da bir onarım
  denemesinden sonra yükseltmeyi tetikler. Mevcut yeniden denenebilir sağlayıcı yedeği kullanılabilir kalır.
- Art arda iki geçersiz araç çağrısı, geçersiz JSON ya da bilinmeyen araç adı gibi nedenlerle ana makine
  doğrulaması tarafından reddedilir.
- Mevcut tekrarlanan çağrı uyarısına ulaşılır ya da sonraki model çağrısı dördüncü model adımıdır.
- Hafif bir yanıt, okuma olmayan bir araç ya da onay gerektiren bir araç ister. Hafif yanıtın tamamı,
  herhangi bir araç gönderilmeden ya da bir onay kartı oluşturulmadan atılır. Yürütücü önceki geçmişi
  alır ve yeniden karar verir. Bu denetim, `use_tool` aracının gerçek hedefini de çözer.
- Bir araç gerçekten çalışmıştır ya da bir kullanıcı etkin turu yönlendirir. Çalışmadan sonraki araç
  hataları da maruz kalma sayılır. Bir korumanın son kapanış yanıtı her zaman yürütücüyü kullanır.

Mevcut izin motoru, yürütücünün ortaya çıkardığı araç çağrısına izin verilip verilmeyeceğine, reddedilip
reddedilmeyeceğine ya da onay gerekip gerekmediğine yine karar verir. Bir model katmanı kararı asla izin
vermez. Araç hazırlama yolu, hafif yanıt üretildikten sonra politika değişmiş olabileceği için yükseltme
koşulunu yeniden denetler.

Yükseltme, yürütücü isteğinden önce `model_fallback` olayını yayar ve maskelenmiş bir
`model.tier_escalated` denetim bilgisi kaydeder. Durum okunamazsa ya da gerekli bir bilgi saklanamazsa
çalışma, sonraki model isteğinden önce başarısız olur. Bu aynı zamanda sürdürmede kalıcı mandaldır:
hafif sağlayıcı hatasının ardından tek bir model adımı içinde kesilmiş bir yürütücü isteği de buna
dahildir. Bir çalışma, yükseltmeden sonra yedek listesi dahil, ayarlanmış hafif modeline asla geri
dönmez. Mevcut kullanım satırları gerçek modeli ve katmanlama açıksa katmanını içerir.

Tamamlanan her model adımı da gerçek katmanını ve modelini saklar. Bekleyen araç çağrıları, yeniden
başlatmada o üreticinin yetkisini korur: hafif modeli kaldırmak ya da değiştirmek ya da bir CLI
yedeğiyle katmanlamayı kapatmak, eski bir hafif kararı yürütücü kararına çeviremez. Böyle bir
yapılandırma değişikliği bekleyen hafif çağrıları atar ve yürütücüye yeniden sorar. Yürütücünün yedek
yoluyla zaten ürettiği bir yanıt standart yanıt olarak kalır.

## Temkinli bağlam sınırları

Temel dalda genel, kalıcı bir çalışma bulaşma kapısı yoktur. Güvenilmeyen sonuçları hafif bir modele geri
göndermemek için bu uygulama, **çalışmış her araç çıktısını yürütücü gerektiren** olarak ele alır ve
dönen metni yorumlamaz. Daha önceki herhangi bir sohbet mesajı olan ya da hafıza çağırma açık olan
yeni bir tur da standart modelle başlar. Araç maruz kalmasının tek istisnası, hiçbir aracın
çalışmadığı ana makine doğrulaması ya da politika reddi sonucudur.

Yalnızca insanın başlattığı üst düzey turlar hafif başlayabilir. Zamanlanmış ve kanal turları standart
başlar. Alt ajanlar mevcut model rolü seçimlerini korur. CLI yürütücüsü, CLI yedeği ya da örtük CLI
varsayılan sağlayıcısı olan botlar katmanlamayı kullanmaz, çünkü bu sağlayıcılar tam bir oturumu yönetir.

Bu sınırlar, bekleyen maruz kalma ve havuzlu karar verici özellikleri birleştirildiğinde daraltılabilir.
Okuma ağırlıklı işlerde ve süren sohbetlerde beklenen tasarrufu azaltırlar. Hafif bir karar atıldığında
yükseltme bir model çağrısı ekleyebilir. Katmanlama daha az çağrı sayısı garanti etmez.

## Doğrulama ve maliyet yorumu

Süreç içi sahte test takımı, etkin yolu, onayları, keşfi, okuma maruz kalmasını, geçersiz çağrıları, tek
yönlü kurtarmayı ve denetim hatalarını sınar. Karışık 20 görevlik gerileme testi, selamları, ipuçlu
okumaları, karmaşık görevleri ve güvenli olmayan hafif önerileri yalnızca yürütücü kullanan bir
başlangıç değeriyle karşılaştırır. Kapı, her iki tarafta 20 betikli sonucun hepsinin geçmesini ve varsayılan
maliyette en az yüzde 20 düşüş olmasını ister. Yapay fiyatlar ve sabit token sayıları kullanır. Yönlendirme
ve denetim davranışını ölçer. Canlı model kalitesini, canlı token tüketimini ya da gerçek sağlayıcı
fiyatlandırmasını ölçmez. Seçtiğin modelleri kendi temsil edici iş yükünde değerlendirene kadar katmanlama
isteğe bağlı kalır.

Denetlenen deneme verisi çalışmasında katmanlamasız 28, katmanlamayla 32 çağrı kullanıldı:

| Deneme verisi grubu | Görev | Yalnızca yürütücü çağrıları | Katmanlı çağrılar |
| --- | ---: | ---: | ---: |
| Tam selamlar | 8 | 8 | 8 |
| İpuçlu okumalar, ortak yönlendirme çağrıları dahil | 4 | 12 | 12 |
| Standart başlayan karmaşık istekler | 4 | 4 | 4 |
| Çalışmadan önce atılan güvenli olmayan hafif yazma önerileri | 4 | 4 | 8 |
| Toplam | 20 | 28 | 32 |

Bu yüzden uygun olan 12 mutlu yol denemesi, her iki tarafta da 20 çağrı kullanır (16 ana model çağrısı
artı mevcut dört yönlendirme çağrısı). Fazladan dört çağrı, dört güvenlik yükseltmesinden gelir: atılan
her hafif yanıtı bir yürütücü kararı izler. Ek bir katman sınıflandırıcı isteği olmamasına rağmen toplam
çağrı sayısı yüzde 14,29 artar. Sağlayıcı yeniden deneme gerilemeleri ayrıca sınanır ve bu 20 denemeye
dahil değildir. Denemenin yapay fiyatlarıyla tahmini maliyet 0,000366 dolara karşı 0,000234 dolardır,
yani yüzde 36,07 düşüş. Her iki tarafta da 20 betikli sonucun hepsi başarılı olmuştur.
