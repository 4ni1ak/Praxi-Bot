# Bir şey olduğunda

Bot panelinde **Otomasyon → Bir şey olduğunda** bölümünü açın. Yeni e-posta,
imzalı çağrı veya zamanlama için kural ekleyin. “İncelemem için yanıt taslağı
hazırla” gibi bir talimat yazın. Kaydedip gözden geçirdikten sonra açın. Yeni
kurallar her zaman kapalı başlar.

Mevcut posta ve talimat zamanlamaları ile imzalı adresler aynı listede görünür. Sessiz saat ve sınır eklemek için birini açın; kaydetmek, kuralı inceleyip yeniden açana kadar durdurur. Gelişmiş iş akışı, ön kontrol ve dosya izleme zamanlamaları Zamanlamalar bölümünde kalır.

E-posta için önce bir IMAP posta kutusu bağlayın ve botun zamanlanmış posta
okumalarına izin verin. Klasörü, göndereni (tam adres, içerir veya alan adı),
isteğe bağlı konu ve ek filtresini seçin. Varsayılan kontrol aralığı iki
dakikadır; bir dakikadan kısa olamaz. İlk kontrol başlangıç noktası oluşturur;
eski postalar çalıştırma başlatmaz. Birlikte gelen postalar tek çalıştırmada
başlık listesi olur. Arama en yeni 100 eşleşmeyle sınırlıdır; teslimat garantisi
vermez. AgentMail gerekli döngü önleme başlıklarını sunmadığı için düzenleyici
yalnızca IMAP kutularını gösterir. Ek filtresi sunucunun MIME yapısı bilgisine
bağlıdır; bilinmeyen bilgi eşleşmiş sayılmaz.

Kutunun kendi adresinden gelen postalar, otomatik yanıtlar, toplu/liste postaları
ve abonelikten çıkma başlığı taşıyan postalar atlanır. Gönderen filtresinde
açıkça belirtilmeyen no-reply adresleri de atlanır. Gönderen filtresi kimlik
doğrulamaz. E-posta sohbet kanalı bağımsızdır: aynı kutu için ikisini birden
açarsanız hem sohbet hem kural çalıştırması başlayabilir.

Çağrılar için **İmzalı adresler** altında adres oluşturup kurala bağlayın.
İmza, tekrar saldırısı koruması ve çağrı sınırları geçerliliğini korur. Bağlı
adres önceki talimatı yerine bu kurala olay ekler. Kural kapalıyken çağrılar
reddedilir; önceki talimata geri dönülmez. Kuralı kaldırmak bağı kaldırır.

Sessiz saatler seçtiğiniz saat dilimini ve yaz saati değişikliklerini izler.
Olayları atlayabilir veya pencere bitince tek özet için saklayabilirsiniz.
Varsayılan sınır kural başına son bir saatte 10, son 24 saatte 50 çalıştırmadır;
botun tüm kuralları için 30 ve 150'dir. Bot için tanımlanan en düşük tavan tüm
kurallarına uygulanır. Fazla olaylar sonraki özette birleşmek üzere bekler.
Kuyruk ve sayaçlar yeniden başlatmada korunur. “Bugün” sayısı UTC'ye göredir.

Her kural en çok 100 olay ve 48 KB saklar. Posta kuyruğu dolarsa veya çalıştırma
başarısız olursa kural duraklar. Art arda on posta kontrolü hatası da duraklatır.
Yeniden açmadan önce etkinliği, bağlantıyı ve filtreleri kontrol edin. Kuralı
düzenlemek bekleyen eski olayları siler ve posta başlangıç noktasını yeniler;
hız sayaçları korunur. Bir grup ayrıldıktan hemen sonra süreç çökerse o grup
kaybolabilir; tam bir kez teslimat garantisi yoktur. Önemli postaları kutuda tutun.

**Örnekle dene**, botun alacağı tam talimatı ve aynı boyut sınırına sahip
korumalı olay metnini gösterir. Model çağırmaz, araç çalıştırmaz. Posta kuralları
yalnızca başlıkları okur; gövdeyi ve ekleri indirmez. Çağrı metni 4.096 karakterle,
son olay bloğu 12 KB ile sınırlıdır. Başlıklardaki görünmez karakterler temizlenir.
Olay metni, talimatınızı yok saymayı istese bile, güvenilmeyen veridir.

Kuralları yalnızca yerel sahip oluşturabilir, düzenleyebilir veya açabilir.
Bot kendi izinleriyle çalışır; dış olay çalıştırmalarında zamanlayıcının tek
seferlik onay tabanı da korunur. Onay gereken işlem bekler ve bildirim gösterir.
Hiçbir işlem otomatik onaylanmaz. Parolalar kurala yazılmaz; posta kutusunun
veya imzalı adresin mevcut yapılandırmasında kalır.
