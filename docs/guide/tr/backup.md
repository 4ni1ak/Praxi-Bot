# Şifreli yedekleme ve geri yükleme

`bot backup`; botlarını, sohbetlerini, hafızanı, becerilerini, iş akışlarını, zamanlamalarını ve
sağlayıcı ayarlarını **parolayla şifrelenmiş tek bir dosyaya** yazar. Bulut sürücüsünde ya da
USB belleğde saklaması güvenlidir ve aynı ya da başka bir makinede geri yükleyebilirsin. Her
güncellemeden önce ve yeni bir bilgisayara geçerken kullan.

```sh
bot backup create ./praxi.bak          # parola sorar (gizli giriş)
bot backup verify ./praxi.bak          # her şeyi çözer ve denetler; hiçbir şeyi değiştirmez
bot backup restore ./praxi.bak         # önce daemon'u durdur
```

## Parola

Parola, komut satırında değer olarak asla kabul edilmez (süreç listesinde ve kabuk geçmişinde
görünürdü). Şu yollardan birini seç:

| Nasıl | Ne için |
| --- | --- |
| Gizli giriş kutusu (varsayılan) | `bot backup create DOSYA` oluştururken iki kez, diğer zamanlarda bir kez sorar |
| Dosya | `--passphrase-file ./pass.txt` (ilk satır; dosyayı gizli tut) |
| Ortam değişkeni | `--passphrase-env MY_BACKUP_PASS` (değişkenin **adı**) |

Yeni bir parola en az 8 karakter olmalıdır. **Kurtarma yolu yoktur:** parola olmadan bir yedek
açılamaz.

## İçinde neler var

Botlar, sohbetler ve geçmiş, hafıza, beceriler, iş akışları, zamanlamalar, izin politikası ve
sağlayıcı/MCP ayarları. Her zaman dışarıda kalanlar: API token'ı, eş kimlik anahtarı, eşleşmiş
cihazlar, tarayıcı profilleri, günlükler ve önceki `backup-*` klasörleri. Yeni bir makinede
geri yükledikten sonra cihazları yeniden eşleştir ve yeni API token'ını kullan.

### API anahtarları (`--include-secrets`)

Anahtarlar, `create` komutuna `--include-secrets` vermedikçe yedekte **bulunmaz**. Bulunduklarında
her şey gibi şifreli saklanır. Öyle olsa bile `restore`, onları gizli bilgi deponya yalnızca
`restore` komutuna da `--include-secrets` verirsen yazar; vermezsen atlanır ve komut kaç tanesinin
atlandığını söyler.

## Oluşturma

`create` var olan bir dosyanın üzerine asla yazmaz, önce bir `.partial` dosyasına yazar ve bitince
adını değiştirir; daemon çalışırken de güvenlidir (SQLite dosyaları çevrimiçi yedekleme API'siyle
kopyalanır). Daemon durdurulmuşken alınan bir yedek tüm dosyalarda tutarlıdır. Büyük veri klasörleri
akış olarak işlenir, belleğe sığmaları gerekmez.

## Doğrulama

`verify` tüm dosyayı çözer ve her dosyayı bildirimle karşılaştırır. Dosya sayısını, boyutu, Praxi
sürümünü ve gizli bilgilerin dahil olup olmadığını bildirir. Veri klasörüne asla dokunmaz ve
çözülmüş veriyi diske yazmaz.

## Geri yükleme

`restore`, veri klasörünü bir daemon tutarken (`bot serve` tarafından tutulan bir `.daemon.lock`
dosyası) ya da servis portunda yanıt verirken çalışmayı reddeder. Herhangi bir şeye dokunmadan
**önce** dosyanın tamamını çözer ve denetler, yani yanlış bir parola ya da hasarlı bir dosya
verini asla değiştirmez. Kurulumda zaten botlar ya da sohbetler varsa `--replace` vermelisin;
değiştirdiği durum, veri klasörünün içindeki bir `backup-*` klasöründe saklanır.
Yedeğin izin politikası mevcut olandan daha serbestse onu kabul etmek için
`--allow-policy-relaxation` kullan.

Bir geri yükleme sürerken yedek dosyasını değiştirme.

## Hasarlı dosyalar ve yanlış parolalar

Yanlış bir parola, değişmiş bir bayt, kesilmiş bir dosya ve sırası değişmiş parçalar aynı iletiyi
verir: *wrong passphrase, or the backup file is damaged or incomplete (nothing was changed)*
(parola yanlış ya da yedek dosyası hasarlı veya eksik, hiçbir şey değiştirilmedi). Kimlik
doğrulama başlığı ve dosyanın her parçasını kapsar, bu yüzden doğrulanmadan hiçbir şeye güvenilmez.

## Biçim (merak edenler için)

Başlıkta `PRAXIBAK` sihirli değeri, sürüm, Argon2id parametreleri (varsayılan olarak 64 MiB, 3 geçiş)
ve rastgele bir tuz bulunur; ardından ChaCha20-Poly1305 ile mühürlenmiş 64 KiB'lık parçalar gelir.
Başlık her parça için doğrulanmış veridir, nonce parça numarasını ve bir son parça bayrağını
kodlar; böylece yeniden sıralama, parça atma, kesme ya da ekleme fark edilir. Yük, `state-bundle`
akışıdır.

## Eski şifresiz yedekler

Şifreli yedeklerden önceki sürümler `bot backup DOSYA` ile şifresiz `.tar` dosyaları yazıyordu.
`bot restore DOSYA` bunları hâlâ geri yükler; bkz. [Yedekleme ve geri yükleme](backup-and-restore.md).
Yeni yedekleri `bot backup create` ile oluştur.

## Ayrıca bak

- [CLI başvurusu](cli-reference.md)
- [Ana makine işlemleri](host-operations.md)
