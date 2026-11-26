# Sunucu ekleme

**Sunucu**, gece gündüz açık duran, genellikle internetten kiralanan başka bir bilgisayardır. Botların orada yaşayabilir, böylece kendi bilgisayarın kapalıyken de çalışmaya devam eder.

**Sunucuya ihtiyacın yok.** Botların zaten bu bilgisayarda çalışıyor. Sunucuyu yalnızca botların 7/24 çalışsın istiyorsan ekle.

Üstteki bilgisayar menüsünü aç ve **Sunucu ekle** seçeneğini seç. İlk ekran bunu iki cümleyle anlatır ve üç büyük seçenek sunar. Her adımda geri dönebilir ya da pencereyi kapatabilirsin. Yeniden açtığında kaldığın yerden devam eder (yalnızca sayfayı hatırlar, adresi, anahtarı ya da kodu asla).

## Üç seçenek

- **Bu bilgisayarda çalıştır (başlamak için önerilir).** Kurulacak hiçbir şey yok.
- **Bir sunucum var.** Tek komut, anahtar dosyası yok. Aşağıya bak.
- **Sunucuya ihtiyacım var.** Üç seçenek için sade bir dille yazılmış, kontrol listeli bir rehber: Oracle Cloud ücretsiz katmanı, herhangi bir sağlayıcıdan kiralanan sunucu ya da evdeki boş bir bilgisayar. Her şeyi sağlayıcının kendi web sitesinde ya da o bilgisayarda yaparsın. Praxi hesabını, parolanı ya da kartını asla görmez.

## Bir sunucum var: tek komut

SSH'e, anahtar dosyasına ya da kendi bilgisayarında bir terminale ihtiyacın yok.

1. Sunucunun **konsolunu ya da terminalini** aç. Bulut sağlayıcılarında sunucunun sayfasında Console, Web console ya da Terminal adlı bir düğme olur. Evdeki bir bilgisayarda Terminal uygulamasını aç.
2. Praxi'de **Komutu kopyala** düğmesine bas ve komutu oraya yapıştır. Komut şunları yapar:
   - GitHub CLI'ın (2.80 ya da daha yenisi) kurulu olduğundan emin olur, çünkü imzaları o denetler;
   - kurulum programını **bu uygulamanın derlendiği sürümün tam olarak aynısı** için indirir;
   - **imzalı derleme kaynağını doğrular** ([Linux sunucuda 7/24 çalışma](linux-host.md) ve [Sürüm yayımlama](release-linux.md) sayfalarındaki denetimin aynısı). Denetim başarısız olursa hiçbir şey kurulmaz;
   - 7/24 çalışan hazır ayarı bir masaüstü oturumuyla kurar ve
   - `bot-pair:` ile başlayan **tek bir eşleştirme kodu** yazdırır.
3. Kodu Praxi'ye yapıştır (konsolun yazdığı her şeyi yapıştırabilirsin, Praxi kodu kendisi bulur) ve **Bağlan** düğmesine bas.

Komutta parola, anahtar ya da herhangi bir token yoktur. Yalnızca sürüm numarası doldurulur. Eşleştirme kodu tek kullanımlıktır, 30 dakika geçerlidir ve yalnızca tek bir masaüstünü eşleştirebilir. Yanlış kod reddedilir, kullanılmayan kod kendiliğinden geçerliliğini yitirir. Yeni bir kod almak için komutu istediğin zaman yeniden çalıştırabilirsin, tekrar etmek güvenlidir.

Ardından Praxi dört canlı denetim gösterir. Bir şey yolunda değilse her birinin yanında sade bir çözüm yazar:

| Denetim | Anlamı | Yeşil değilse |
| --- | --- | --- |
| Sunucu yanıt veriyor | Bu uygulama sunucuyla konuşabiliyor | Sunucunun açık ve çevrimiçi olduğundan emin ol. Eski kod kullanıldıysa yeni bir kod al |
| Aynı Praxi sürümü | Sunucu ve uygulama aynı sürümü çalıştırıyor | Komutu sunucuda yeniden çalıştır |
| İzlenecek bir ekran | Sunucuda izleyebileceğin bir masaüstü var | Eklemek için komutu yeniden çalıştır. Botlar bunsuz da çalışır |
| Modeller gönderilebiliyor | Bu uygulama model anahtarlarını sunucuya gönderebilir | Sunucuda: `bot host admin --all-desktops` |

Seni yalnızca ulaşılamayan bir sunucu engeller, diğer üçü tavsiyedir.

Elinde zaten bir eşleştirme bileti varsa (örneğin sunucuda `bot host invite --role desktop` ile aldığın), pencerenin altındaki **Bunun yerine eşleştirme biletim var** seçeneğini kullan.

### Sunucuda (yöneticiler için)

Komut, sürüm etiketi doldurulmuş `crates/host-provision/scripts/bootstrap.sh` betiğidir ve `bash -s` ile çalışır. Yalnızca bileti yazdıran `bot host invite --role desktop --ttl-secs 1800 --ticket-only` komutunu kullanır. İki seçenek elle de çalışır. Resmî bir sürüm olmayan bir derleme uyarı gösterir, çünkü sürümü yayımlanmamış olabilir.

## Sunucuya ihtiyacım var

Bir kart seç. Her rehber sade adımlardan oluşan kısa bir kontrol listesidir ve yukarıdaki tek komut sayfasına çıkan “Sunucum hazır” ile biter.

### Oracle Cloud ücretsiz katmanı, kısaca

1. oracle.com/cloud/free adresinde ücretsiz bir hesap aç (kart, insan olduğunu kanıtlamak içindir. Ücretsiz katmandan ücret alınmaz).
2. Konsolda Compute, sonra Create instance seç. Ubuntu 22.04 ya da daha yenisini ve Always Free işaretli bir şekil (shape) seç.
3. Ağ kurallarını olduğu gibi bırak.
4. Örnek Running durumuna geçince **Console connection** bağlantısını ya da Cloud Shell'i aç ve Praxi'deki komutu yapıştır.

### Kiralanan sunucu

Ubuntu 22.04 ya da daha yenisini sunan her sağlayıcı olur (yaklaşık 2 GB bellek ve 10 GB disk). Sunucuyu oluştur, web konsolunu aç ve komutu yapıştır.

### Evdeki boş bir bilgisayar

Ubuntu 22.04 ya da daha yenisini kur, bilgisayarı açık ve çevrimiçi tut, Terminal uygulamasını aç ve komutu yapıştır.

## Gelişmiş: SSH ile bağlan

Sunucuna zaten bir SSH anahtar dosyasıyla ya da ajanıyla giriyorsan, “Bir sunucum var” sayfasında **Gelişmiş: SSH ile bağlan** bölümünü aç. Bu eski yoldur ve yeni başlayanların hiçbir zaman ihtiyaç duymayacağı bir yoldur.

- **Sunucu adresi**, örneğin `203.0.113.7`. `ssh://` ya da `user@` yazma.
- **Kullanıcı**: Ubuntu imajlarında `ubuntu`, Oracle Linux'ta `opc`.
- **Anahtar dosyası** ya da SSH ajanın. Bu bilgisayarda bulunan anahtar dosyaları **Bu bilgisayarda bulunan anahtarlar** altında listelenir. Senin yerine hiçbiri seçilmez ve yalnızca yol kullanılır. Anahtar asla kopyalanmaz ya da saklanmaz. Praxi parola kullanmaz.
- **Gelişmiş**: port (neredeyse her zaman 22).

Praxi bir kontrol listesi gösterir (sunucu yanıt veriyor, giriş çalışıyor, sistem destekleniyor, yeterli bellek ve disk var, ağ ve portlar uygun), sunucunun parmak izini onaylamanı bir kez ister, gerekirse Praxi'yi kurar ve eşleştirir.

Sık görülen mesajlar:

| Mesaj | Ne demek | Çözüm |
| --- | --- | --- |
| Sunucu anahtarını kabul etmedi | “permission denied (publickey)” | Kullanıcı adını denetle, bu sunucuya ait anahtarı seç |
| Anahtar dosyan başkaları tarafından okunabiliyor | Anahtar dosyasının izinleri fazla açık | Mac/Linux: `chmod 600 <key file>`. Windows: Özellikler, Güvenlik, yalnızca kendi kullanıcını bırak |
| Bu anahtar bir parolayla kilitli | Praxi parolayı yazamaz | `ssh-add` ile kilidini aç ve SSH ajanını seç |
| Sunucu zamanında yanıt vermedi | Büyük olasılıkla 22 numaralı port kapalı | Sağlayıcının güvenlik duvarında 22 numaralı porta gelen TCP'ye izin ver |
| Sunucu bağlantıyı reddetti | SSH servisi çalışmıyor | `sudo systemctl start ssh` |

## Sunucuların

Bilgisayar menüsündeki **Sunucuların** bölümü, her sunucuyu durumuyla listeler. **Kaldır** sunucuyu yalnızca bu bilgisayarda unutturur. Sunucuya, üzerindeki botlara ve dosyalara dokunulmaz, çalışmaya devam ederler. İstersen sonra yeniden ekleyebilirsin.

## Sunucuyu güncelleme

**Sunucuların** (ve sunucu seçiliyken Bilgisayar menüsü) her sunucunun sürümünü bu uygulamanınkinin yanında gösterir. Sunucu daha eskiyse **Güncelleme var** yazısını görürsün. **Yeniden kontrol et** sunucuya bir kez daha sorar. Terminal ya da başka bir özellik sunucunun daha eski bir Praxi-Bot çalıştırdığını söylerse, aynı **Sunucuyu güncelle** düğmesi orada da sunulur.

**Sunucuyu güncelle** önce ne olacağını söyler: sunucu birkaç saniyeliğine yeniden başlar, orada çalışan botlar duraklar ve kaldığı yerden devam eder, verilerin ve ayarların korunur, önceki sürüm de sunucuda kalır (`/opt/praxi.backup-<time>`, yalnızca en yenisi). Sonra sunucuyu eklerken kaydedilen giriş bilgileriyle bağlanır (yeni hiçbir şey saklanmaz), verinin bir kopyasını alır ve `bot host upgrade` ile aynı doğrulanmış güncellemeyi çalıştırır: yalnızca bu uygulamayla eşleşen sürüm, imzası denetlendikten sonra kurulur. Bir şey ters giderse kurulum programı önceki sürümü geri koyar ve uygulama ne olduğunu, ne yapman gerektiğini sade bir dille anlatır. **Ayrıntıları göster** her adımı listeler.

Sunucu başka bir yolla eklendiyse (bu bilgisayarda kayıtlı giriş yoksa) ya da bu bilgisayar SSH kullanamıyorsa, uygulama onun yerine sunucuya yapıştıracağın komutların aynısını gösterir: [Linux sunucu paketlerini yayımlama](release-linux.md) sayfasındaki doğrulanmış bootstrap betiği, `--update --version <tag>` ile. Güncellemeyi yalnızca bilgisayarın sahibi başlatabilir. Eşleştirilmiş bir telefon ya da bilgisayar başlatamaz.

## Bugünkü sınırlar

- Praxi'yi kurmak, uygulamanla aynı sürümün yayımlanmış bir sürümünü gerektirir. Geliştirme derlemesi uyarı gösterir.
- Sunucunun github.com adresine giden internet erişimi, systemd ve Ubuntu 22.04 ya da daha yenisi (ya da yakın tarihli bir Oracle Linux / Fedora ailesi sistemi) olmalı.
- Güncelleme, “Sunucu ekle” sihirbazının kaydettiği SSH girişini gerektirir. Girişsiz güncelleme henüz yok. O olmadan güncelleme komutunu sunucuya yapıştır (yukarıya bak).
