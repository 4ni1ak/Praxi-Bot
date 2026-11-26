# Bilgi gömme kurulumu

Bilgi araması, bir gömme sağlayıcısı olmadan da yerelde çalışır. Uzak gömmeler ve yerel doğal
çıkarım isteğe bağlı ana makine yetenekleridir. Kaynak yönetimi ve sahip onayı arayüzü, RAG
yığınının kaynaklar katmanında gelir. Gömme katmanı, o yollar için aynı ana makine servislerini sunar.

## Uzak sağlayıcılar ve sahip onayı

Sağlayıcıyı ve onun SecretRef değerini normal sağlayıcı ayarlarından yapılandır. Modelini, uç nokta
kimliğini ve boyutlarını botun gömme ayarlarında seç. Bir sağlayıcı seçmek, belge ya da sorgu
metninin ana makineden çıkmasına yetki vermez. Daemon, taşınabilir bir `account_group` değerini
yok sayar ve ortak kota anahtarını sağlayıcının kimlik bilgisi başvurusundan (ya da kimlik
doğrulaması olmayan bir uç noktadan) türetir. Aynı kimlik bilgisi başvurusunu kullanan botlar aynı
defteri paylaşır.

Sahip, belge çıkışını **her kaynak** için ayrı ayrı onaylar. Sorgu çıkışının ise bot genelinde ayrı bir
onayı vardır, çünkü bir sorgu, bir kaynakla ilgisi olmayan özel sohbet metni içerebilir. Onay kartı
sağlayıcıyı, modeli, hedefi ve kaynağı belirtir. Onay istekleri beş dakika sonra sona erer, yalnızca
bir onay kabul eder ve değişmiş yapılandırmayı ya da kaynak sürümünü reddeder. Genel araç izinleri ve
JSON onay alanları bu yetkiyi veremez.

Gömme, izin motorunun mevcut `Remote` kategorisini kullanır. Genel/bot geçersiz kılma kuralları
altında etkin bir `Remote: deny`, bir politika kilidi ya da desteklenmeyen bir sınır, onayı ve
gönderimi engeller. Bu kategori uzak bilgisayar erişimini de yönettiği için onu değiştirmek iki
özelliği de etkiler. Sorgu gönderimi, gerçek çalışmanın Human, Schedule ya da Channel başlatıcısını ve
devralınan alt ajan politikasını kullanır. Bir Human kurulum onayı, zamanlanmış ya da kanal Deny
kararını geçersiz kılmaz. Ana makine, onaylanmış izni bot yapılandırmasının dışında,
`knowledge-authority.db` içinde tutar.

Bir modeli değiştirmek yeni onay gerektirir. Eski ücretli vektörler, başarılı bir yedek tamamlanana
kadar etkin olmayan bir nesil olarak kalır. Farklı bir profil için sorgulara sessizce yanıt veremezler.
Bir kaynağı açıkça iptal etmek ya da devre dışı bırakmak, onun uzak vektörlerini siler. İçe aktarma
kaynakları askıya alır, içe aktarılan tüm vektör/önbellek durumunu siler ve dizin kimliğini döndürür.
Böylece eski ana makine izinleri, aynı bot/kaynak UUID'leriyle bile yeniden oynatılamaz.

Sağlayıcının 429 yanıtları, son deneme haklarını tüketmeden işi erteler. Gerçekleşen her HTTP denemesi
yine de yerel kotayı tüketir. Sağlayıcı geçersiz ya da aşırı bir yeniden deneme gecikmesi gönderirse
kalıcı bekleme süresi 48 saatle sınırlanır. Sahibin bekleme sıfırlama işlemi, o kimlik bilgisini
kullanan botlarda sağlayıcı bekleme sürelerini temizler ve kota defterini korur. Girdi reddi,
atlanan parçaları ayırıp kaydeder, böylece sağlam parçalar devam edebilir. Açık yeniden deneme,
kaynağın atlanan parça ve hata durumunu temizler.

## Yerel doğal profil kaydı

Daemonu `local-embeddings` özelliğiyle derle. Çok dilli E5 small modelini, tokenizer JSON/yapılandırma
dosyalarını ve uyumlu bir ONNX Runtime kitaplığını, bütünlüğünü ve lisansını doğruladığın kaynaklardan
kur. Daemon eksik parçaları indirmez. Linux ve Windows'ta desteklenen değişmez çalışma anı anlık
görüntüleri vardır. Yerel doğal yükleme başka işletim sistemlerinde güvenli biçimde başarısız olur.

`knowledge-host.json` dosyasını doğrudan daemonun veri klasöründe oluştur. Bu, sistemi işleten kişinin dosyasıdır,
bir bot profili ya da HTTP ile düzenlenebilen bir ayar değildir. Unix'te daemon kullanıcısına ait olmalı
ve grup ya da diğerleri tarafından yazılabilir olmamalıdır (`0600` kipi uygundur). Aşağıdaki yollar
yalnızca biçimi gösterir. Her yolu ve her `REPLACE_WITH_64_HEX_SHA256` metnini gerçek mutlak yol ve
SHA-256 ile değiştir. Windows'ta `C:\\PraxiModels\\onnxruntime.dll` gibi mutlak yollar kullan.

```json
{
  "profiles": {
    "e5-small": {
      "label": "Multilingual E5 small on this host",
      "artifacts": {
        "runtime": {
          "path": "/opt/praxi-models/onnxruntime/libonnxruntime.so",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "model": {
          "path": "/opt/praxi-models/e5-small/model.onnx",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "tokenizer": {
          "path": "/opt/praxi-models/e5-small/tokenizer.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "config": {
          "path": "/opt/praxi-models/e5-small/config.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "special_tokens_map": {
          "path": "/opt/praxi-models/e5-small/special_tokens_map.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        },
        "tokenizer_config": {
          "path": "/opt/praxi-models/e5-small/tokenizer_config.json",
          "sha256": "REPLACE_WITH_64_HEX_SHA256"
        }
      }
    }
  }
}
```

Kayıt defteri en fazla 16 profil kimliği kabul eder. Her biri 1 ile 128 arasında ASCII harf, rakam,
alt çizgi ya da kısa çizgiden oluşur. Arayüz yalnızca kimliği, etiketi ve türetilmiş model profilini
alır. Bot `local_profile_id: "e5-small"` değerini seçer. Bot JSON'u hiçbir parça yolunu ya da özetini
geçersiz kılamaz. Kayıt defteri açılırken sondaki bir sembolik bağ/yeniden ayrıştırma noktası
reddedilir ve denetimler, baytları okunan tutamaç üzerinde yapılır.

Model/tokenizer dosyaları, doğrulanmış sahipli arabelleklere okunur. Yerel çalışma zamanı Linux'ta
mühürlü bir memfd içine, Windows'ta ise yazma/silme paylaşımına karşı tutulan özel bir dosyaya
kopyalanır. Yerel yükleme bu anlık görüntüyü kullanır, özgün yol adını asla kullanmaz. Süreç,
doğrulanmış çalışma zamanı tutamacını ömrü boyunca elinde tutar. Kurulu özgün dosyaları değiştirmek,
zaten kabul edilmiş baytları değiştiremez. Yerel çalışma zamanı neslini değiştirdikten sonra yeniden
başlat. Çıkarım tek bir işlemci iş parçacığı ve tek bir paylaşılan çıkarım yuvası kullanır, parti başına
iki pencere ve girdi başına en fazla sekiz adet 512 tokenlık pencere ile çalışır. Aşırı büyük girdiler
sessizce kırpılmaz, atlanan parçalar olarak ayrılır.

## Veri kümesi sınırları ve doğrulama

Kesin tarayıcı, bir sorguyu yalnızca vektör sayısını, boyutları ve tahmini baytları denetledikten sonra
kabul eder. Varsayılan 500 ms bütçeyle bayt sınırı 16.384.000'dir: 384 boyutta 10.666 vektör, 1.536
boyutta 2.666 ya da 3.072 boyutta 1.333 vektör. Bunlar ayrıca ayarlanmış satır sınırıyla da
kısıtlanır. Daha büyük uygun kümeler, uzak sorgu isteği harcanmadan sözcük tabanlı aramaya döner. Bu
tavanlar işi sınırlar, her makinenin 500 ms içinde bitireceğini vaat etmez.

CI, çözümlenmiş yerel özellik grafiğinin model merkezi ve ONNX ikili indirme özelliklerini
dışarıda bıraktığını doğrular. Cargo bağımlılıklarını ayrıca indirir, sonra yalıtılmış testleri Cargo
çevrimdışı kipinde ve desteklenen ONNX çevrimdışı/indirmeyi atla bayraklarıyla çalıştırır. Bu, bir
işletim sistemi ağ yalıtımı sağlamaz. CI deneme verileri özellik bağlantılarını, bayt sabitlemeyi,
tokenizer/pencere davranışını ve iptali doğrular. Gerçek model kalitesi ve makineye özgü çıkarım hızı,
sistemi işleten kişinin kurduğu parçalarla yapılacak ölçümler gerektirir.
