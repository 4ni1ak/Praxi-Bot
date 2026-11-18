# Türkçe ve İngilizce çeviri kuralları

## Çeviri anahtarları

Türkçe çeviri anahtarları ile İngilizce anahtarlar aynı yapıya sahip olmalıdır.
Eksik i18n anahtarı denetimi, kaynak yönetimi ekranının iki dilde de tamamlandığını
doğrular. Kullanıcıya gösterilen cümleleri bileşen içinde birleştirmek yerine tam
anlamlı çeviri girdileri kullanılır. Sayı, tarih ve dosya adı ayrı parametrelerdir;
yer tutucular her iki dilde de aynı isimlerle tanımlanır.

## Kaynak ekranı metinleri

“İndeksleniyor”, “hazır”, “duraklatıldı” ve “yeniden izin gerekli” durumları aynı
anlama gelmez. Özellikle bir arşivden alınan klasörün eski bilgisayardaki yolunu
yeni bilgisayarda hemen okumamak gerekir. Yeniden bağlama düğmesi kullanıcının
seçtiği konumu açıkça gösterir. Embedding sağlayıcısına metin gönderme izni ayrı
bir açıklama ve ayrı bir seçim olarak sunulur.

| Türkçe | English | Anlam |
| --- | --- | --- |
| Kaynak ekle | Add source | Kullanıcı bir kaynak seçer |
| Yeniden indeksle | Reindex | Seçilmiş içerik tekrar işlenir |
| İzni kaldır | Revoke consent | Gelecekte dışarı metin gönderilmez |
| Kaynağı sil | Delete source | Türetilmiş indeks satırları temizlenir |

## Dil ve yerel ayar

Türkçe büyük İ ile küçük i dönüşümleri arama örneklerinde yer alır. Kullanıcının
belgelerini İngilizceye çevirip saklamak zorunlu değildir; asıl metin ve kaynak
konumu korunur. Arayüz tarihi yerel ayarla biçimlendirir, veritabanındaki zamanlar
ise açıkça belirtilmiş ortak bir zaman tabanında tutulur. Çeviri metinleri teknik
hata yığınlarını veya gizli sağlayıcı anahtarlarını kullanıcıya taşımamalıdır.

Yeni bileşenler dört temada kontrol edilir. Uzun Türkçe kelimeler dar pencere
genişliğinde taşma yapmamalı, onay düğmesinin anlamı kesilerek kaybolmamalıdır.
