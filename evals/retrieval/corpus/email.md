# E-posta kaynak sınırı

## Fatura klasörü

Bu deneme hesabında bot yalnızca Faturalar klasöründeki seçilmiş e-posta
iletilerini okuyabilir. İleti kimliği klasör, UIDVALIDITY ve UID birlikte
kullanılarak oluşturulur. Sadece görünen sıra numarasını saklamak yeterli
değildir; sunucu iletileri yeniden sıraladığında başka bir e-posta seçilebilir.
Kaynak izni kontrol edilmeden gövdeyi okuyup daha sonra sonucu gizlemek doğru
bir erişim sınırı oluşturmaz.

## Ekler ve içerik

Fatura eki bir belge olarak ayrıştırılır, içindeki metin komut olarak çalıştırılmaz.
HTML e-posta gövdesi okunabilir metne dönüştürülürken izleme görselleri ve uzak
kaynaklar indirilmez. E-postanın gönderen alanı veya belge başlığı güven düzeyini
değiştiremez. Kaynak kaydı, botun hangi klasöre erişebildiğini host tarafından
doğrulanan bir kimlikle bağlar.

## İzin kaldırma

Kullanıcı kaynak iznini kaldırdığında bekleyen indeksleme işleri durdurulur.
Çalışmaya daha önce başlamış bir ayrıştırıcı sonucu yayımlamadan önce kaynak
sürümünü tekrar kontrol eder. Silinen kaynağın parçaları arama sonuçlarında
görünmemelidir. Arşivden alınan eski bir izin yeni bilgisayarda kendiliğinden
etkinleşmez; kullanıcı hesabı ve klasörü yeniden doğrular.

## Kapsam dışında

Bu kaynak türü e-posta göndermeye izin vermez. Bir fatura hakkında özet istemek,
ödeme yapmak veya gönderen kişiye mesaj yazmak için otomatik yetki anlamına
gelmez. Arama alıntıları, gövdedeki doğrulanmamış bağlantıları güvenilir uygulama
komutları gibi sunmaz. Sonuçta kaynak adı, belge konumu ve içerik tarihi
gösterilerek insanın asıl kaydı kontrol etmesi sağlanır.
