# Windows'a kurulum

Bu sayfa Windows yükleyicisini ayrıntılı anlatır: neyi kurduğunu, verilerinin nerede durduğunu, yükseltme ve kaldırmanın nasıl çalıştığını, bir şey ters giderse ne yapacağını. İlk açılış sihirbazı için [Kurulum ve ilk çalıştırma](install.md) sayfasına bak.

## Başlamadan önce

- Windows 10 ya da 11, 64 bit. Yönetici hakkı gerekmez, çünkü yükleyici yalnızca senin hesabın için kurar.
- **WebView2.** Uygulama penceresini Microsoft Edge WebView2 çizer. Güncel Windows 10/11'de zaten vardır. Yoksa yükleyici küçük WebView2 kurulum aracını indirir (bir kez internet bağlantısı gerekir). Uygulama penceresi boş kalırsa Microsoft'tan "Evergreen Runtime"ı kur ve Praxi'yi yeniden başlat.
- **SmartScreen.** Sürümler henüz kod imzalı değil, bu yüzden Windows "Windows bilgisayarınızı korudu" uyarısı gösterebilir. **Daha fazla bilgi**'yi, sonra **Yine de çalıştır**'ı seç. Güvende olmak için indirdiğin dosyayı çalıştırmadan önce denetle: SHA-256 değerini sürüm sayfasındaki `SHA256SUMS.txt` ile karşılaştır ve dosyayı kimin ürettiğini GitHub'a doğrulat. Tek bir betik ikisini de yapar, [İndirdiğini denetle](verify-download.md) sayfasına bak.

## Neler kurulur

| Ne | Nerede |
|------|-------|
| Program dosyaları (`bot-desktop.exe`, yardımcı servis olarak `bot.exe` daemon) | `%LOCALAPPDATA%\Programs\Praxi-Bot` |
| Verilerin: `bot.db`, `config`, günlükler, `backups` | `%APPDATA%\praxi\praxi-bot` |
| API anahtarları | Windows Kimlik Bilgileri Yöneticisi (hiçbir zaman dosyalarda değil) |
| Başlat menüsü kaydı | `Praxi-Bot` |

Oturum açınca otomatik başlatma varsayılan olarak kapalıdır. **Ayarlar**'dan açabilirsin (ya da yükleyiciye `/AUTOSTART=1` verebilirsin). Uygulamanın aynı anda yalnızca bir kopyası çalışır: onu yeniden başlatırsan var olan pencere öne gelir.

## Pencere denetimleri ve görünüm

Windows'ta masaüstü uygulaması varsayılan olarak sistemin pencere çerçevesini kapatır ve küçült, büyüt/eski boyuta getir ve kapat düğmelerini kendisi çizer. Bu düğmeler, ayrılmış başlık çubuğu şeridinin sağ üst köşesindedir. Kenar çubuğu ile sağdaki bot paneli bu şeridin altından başlar, yani düğmelerin üstünü hiç örtmez. Pencereyi taşımak için şeridi sürükle, büyütmek için çift tıkla. Uygulamanın çalıştırılabilir dosyası, penceresi ve NSIS yükleyicisi de tam Praxi hayalet simgesini kullanır.

- **Sistemin başlık çubuğuna geri dön**: standart işletim sistemi pencere kenarlarını tercih edersen **Ayarlar → Görünüm** sayfasında **Pencere çerçevesi**'ni aç. Windows dışındaki sistemlerde sistemin çerçevesi varsayılan olarak açık kalır.
- **Duvar kâğıdı perdesi ve bulut hareketi**: duvar kâğıdı açıkken (**Ayarlar → Görünüm → Duvar kağıdı**) **Duvar kağıdı perdesi** kaydırıcısı (yüzde 0 ile 100 arası, varsayılan yüzde 35), temaya göre renklenen yumuşak bir perdenin bütün duvar kâğıdını ne kadar kapatacağını belirler. Duvar kâğıdı pencerenin her yerinde arkada görünür kalır. Mesajlar açık renkli zeminlerin üstünde durur, bu yüzden yazı, kaydırıcı hangi değerde olursa olsun en az 4,5:1 kontrastını korur. **Bulut hareketi** (**Kapalı** / **Hafif** / **Canlı**, varsayılan **Hafif**) ile sabit bir kare, sakin ve 4 saniyede 1 piksellik bir kayma ya da en çok 30 kare/sn ile çalışan katmanlı WebGL bulutları arasında seçim yaparsın. Bulutlar pencere gizliyken, odakta değilken, pil tasarrufu açıkken ya da azaltılmış hareket isteniyorken durur, WebGL yoksa sabit duvar kâğıdına döner. [Canlı bulut duvar kâğıdı ölçümleri](../../design/live-cloud-wallpaper-measurement.md) sayfasına bak.
- **Sınırlar**: bütünleşik pencere denetimleri yalnızca masaüstü uygulamasının içinde çalışır, arayüzü tarayıcıda önizlerken çalışmaz. Bu yalnızca görsel bir pencere çerçevesidir ve daemon izinlerini değiştirmez.

## Yükseltme

Yeni kurulum `.exe` dosyasını eskisinin üstüne çalıştır. Çalışan Praxi işlemleri önce durdurulur ve verilerin korunur. Yeni sürüm daha yeni bir veritabanı şeması istiyorsa uygulama önce veritabanının bir kopyasını `%APPDATA%\praxi\praxi-bot\backups\bot-v<old>-<timestamp>.db` yoluna yazar (en yeni üçü saklanır). Geri dönmek için Praxi'yi kapat, eski sürümü kur ve yedeği `bot.db` üzerine kopyala.

## Kaldırma

**Ayarlar, Uygulamalar**'ı (ya da Başlat menüsü kaydını) kullan. Kaldırıcı çalışan `bot-desktop.exe` ve `bot.exe` işlemlerini durdurur ve otomatik başlatma kayıt defteri değerini siler. Sohbetlerin, botların ve ayarların (`%APPDATA%\praxi\praxi-bot` ile `%LOCALAPPDATA%\praxi\praxi-bot` içinde) açıkça **Uygulama verilerini de sil**'i işaretlemediğin sürece korunur. Kimlik Bilgileri Yöneticisi'ndeki anahtarları kaldırıcı silmez. Gitmesini istiyorsan oradaki `Praxi-Bot` kayıtlarını kendin sil.

## Bir şey ters giderse

- Uygulama, nedeni ve tanılama bilgisini kopyalamak için bir düğmesi olan bir açılış sorunu ekranı gösterir. Hata bildirirken o metni ekle (içinde anahtar yoktur).
- Günlükler: `%APPDATA%\praxi\praxi-bot\desktop-daemon.log` (önceki çalışma `desktop-daemon.log.1` içinde).
- "Address already in use": başka bir program daemon portunu kullanıyor. Onu kapat ya da Görev Yöneticisi'nde başıboş kalmış bir `bot.exe`'yi sonlandır, sonra Praxi'yi yeniden başlat.
- Antivirüs `bot.exe`'yi karantinaya aldı: dosyayı geri yükle ve kurulum klasörünü istisna olarak ekle. İmzasız dosyalar bazen yanlış işaretlenir.

Ayrıca [Sorun giderme](troubleshooting.md) sayfasına bak.
