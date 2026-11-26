# Praxi'yi MCP sunucusu olarak kullanma

`bot mcp serve`, seçtiğin tek bir Praxi botunu dışarıdaki bir MCP istemcisine stdin/stdout
üzerinden açar. Her zamanki Praxi daemonu çalışır durumda kalmalıdır. MCP süreci, daemonun
yalnızca bu bilgisayarda dinleyen HTTP kapısına, gizli bilgi deposunda duran kapsamı sınırlı
bir kimlik bilgisiyle bağlanır. Daemonun `api-token` dosyasını okumaz.

## Kimlik bilgisi oluştur ve bağlan

Botun depolama UUID'sini `bot bots list` ile bul, sonra şu komutları yerel sahip olarak çalıştır.
Örnekteki UUID'yi kendi botunun gerçek UUID'siyle değiştir.

```sh
bot token create --scope mcp:tasks --bot 00000000-0000-0000-0000-000000000001 --ttl 30d
bot mcp serve --bot 00000000-0000-0000-0000-000000000001
```

İlk komut herkese açık bir kimlik bilgisi `id`'si, izin verilen botları ve kapsamları ve
`expires_at_ms` değerini yazdırır. Bearer değerini kendi içinde saklar ve onu asla yazdırmaz.
İkinci komut stdout üzerinden MCP konuşur. Etkileşimli bir terminalde kullanman için değil,
MCP istemcinin başlatması için tasarlanmıştır.

Seçtiğin bota ve katmanlara uyan birden fazla etkin kimlik bilgisi varsa herkese açık kimliği
kendin ver:

```sh
bot mcp serve --bot 00000000-0000-0000-0000-000000000001 --token-id 00000000-0000-0000-0000-000000000002
```

Örneğin sahibinin elle yapacağı Claude Code denemesi, stdio komutunu şöyle kaydedebilir:

```sh
claude mcp add praxi -- bot mcp serve --bot 00000000-0000-0000-0000-000000000001 --token-id 00000000-0000-0000-0000-000000000002
```

Daemonla aynı veri klasörünü ve aynı gizli bilgi arka ucunu kullan. İşletim sisteminin
anahtarlığına, komutu başlatan süreç erişebilmelidir. Şifreli bir dosya kullanıyorsan yerleşik
`BOT_SECRETS_PASSPHRASE` ortam ayarı işe yarar. MCP kipi bu parolayı stdin üzerinden asla
sormaz. Kimlik bilgilerini istemci yapılandırma dosyalarına koyma. İstemci yapılandırması
yalnızca herkese açık bot kimliğine ve token kimliğine ihtiyaç duyar.

`--url` yalnızca HTTP loopback adresini kabul eder. Bu bağlantıda yönlendirmeler ve HTTP
vekilleri kapalıdır. `--remote` desteklenmez. Bu özellikte gelen bağlantıları dinleyen bir MCP
HTTP kapısı yoktur.

## MCP istemcini bağla

`bot mcp serve` bir **stdio** sunucusudur. Aşağıdaki her istemci onu bir alt süreç olarak
başlatır ve stdin/stdout üzerinden satır satır JSON-RPC konuşur. MCP protokolünün 2024-11-05,
2025-03-26, 2025-06-18 ve 2025-11-25 sürümlerini konuşur (daha yeni bir sürüm isteyen istemci
2025-11-25 alır), yalnızca `tools` yeteneğini bildirir ve tek sayfalık bir araç listesi sunar
(`nextCursor` yoktur).

Her tarifin ihtiyacı olanlar:

- **Komut.** `bot` ve şu bağımsız değişkenler:
  `mcp serve --bot <BOT-UUID> --token-id <CREDENTIAL-ID>`. **Bot seçen
  bağımsız değişken** `--bot`'tur. Değeri, `bot bots list` çıktısındaki depolama UUID'sidir. Bir
  sunucu süreci tam olarak bir bota hizmet eder. `--allow tasks,computer` seçeneğini yalnızca
  bilgisayar katmanı için ekle. `--data-dir <PATH>` ya da
  `--secrets-backend keychain|file` gibi genel bayraklar `mcp`'den **önce** gelir.
- **`bot` için mutlak yol** (örneğin `C:\Tools\bot.exe` ya da `/usr/local/bin/bot`). Menüden ya da
  dock'tan açılan masaüstü istemcileri kabuğunun `PATH` değerini miras almaz.
- **Token, güvenli biçimde.** Bearer değeri hiçbir zaman istemci yapılandırmasının parçası olmaz.
  `bot token create` onu gizli bilgi deposuna koyar ve yalnızca herkese açık bir kimlik bilgisi
  kimliği yazdırır. Yapılandırmada `--bot` ve `--token-id` bulunur, ikisi de herkese açıktır.
  İşletim sisteminin anahtarlığıyla dosyada hiçbir gizli bilgi gerekmez. Yalnızca şifreli dosya
  deposunda süreç ortamında `BOT_SECRETS_PASSPHRASE` gerekir. Bunu istemcinin `env` bölümüne
  yalnızca yapılandırma dosyası sana özelse koy. Daha iyisi, istemciyi bu değişkeni zaten
  dışa aktarmış bir kabuktan başlat. Bir bearer değerini ya da daemonun `api-token` dosyasını
  asla bir istemci dosyasına yapıştırma.

Örneklerde `<BOT-UUID>` ve `<CREDENTIAL-ID>` yer tutucu olarak kullanılır.

### Claude Code

Resmî [Claude Code MCP rehberine](https://code.claude.com/docs/en/mcp) bak. Seçenekler sunucu
adından önce gelir, `--` ise komutu ayırır:

```sh
claude mcp add --scope user praxi -- bot mcp serve --bot <BOT-UUID> --token-id <CREDENTIAL-ID>
```

`--scope local` (varsayılan, bu proje ve sen), `project` (proje kökünde paylaşılan bir
`.mcp.json` yazar, yalnızca herkese açık kimlikleri commit et) ve `user` (tüm projelerin,
`~/.claude.json` içinde saklanır). Eşdeğer `.mcp.json`:

```json
{
  "mcpServers": {
    "praxi": {
      "type": "stdio",
      "command": "bot",
      "args": ["mcp", "serve", "--bot", "<BOT-UUID>", "--token-id", "<CREDENTIAL-ID>"]
    }
  }
}
```

`claude mcp list` ya da `/mcp` ile kontrol et. Claude Code'un MCP başlatma ve araç çağrısı
zaman aşımları `MCP_TIMEOUT` ve `MCP_TOOL_TIMEOUT` ortam değişkenleriyle uzatılabilir. Bu,
onay bekleyen çağrılar için önemlidir.

### Claude Desktop

Resmî [yerel MCP sunucusu hızlı başlangıcına](https://modelcontextprotocol.io/docs/develop/connect-local-servers)
bak. Settings, Developer, Edit Config yolunu izle ya da `claude_desktop_config.json` dosyasını
doğrudan düzenle:

| İşletim sistemi | Konum |
| --- | --- |
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` |
| Linux | `~/.config/Claude/claude_desktop_config.json` |

```json
{
  "mcpServers": {
    "praxi": {
      "command": "/usr/local/bin/bot",
      "args": ["mcp", "serve", "--bot", "<BOT-UUID>", "--token-id", "<CREDENTIAL-ID>"]
    }
  }
}
```

Windows'ta ters eğik çizgileri JSON içinde ikiye katla (`"C:\\Tools\\bot.exe"`). Her
düzenlemeden sonra Claude Desktop'ı yeniden başlat.

### Cursor

Resmî [Cursor MCP kurulum rehberine](https://prod.cursor.com/help/customization/mcp) bak.
Genel dosya `~/.cursor/mcp.json` (Windows'ta `%USERPROFILE%\.cursor\mcp.json`), proje içindeki
dosya ise `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "praxi": {
      "command": "bot",
      "args": ["mcp", "serve", "--bot", "<BOT-UUID>", "--token-id", "<CREDENTIAL-ID>"]
    }
  }
}
```

Dosyayı kaydet, Cursor'ı yeniden başlat, sonra sunucuyu görmek için Customize → MCPs bölümünü aç.

### VS Code (GitHub Copilot, agent kipi)

Resmî [VS Code MCP yapılandırma başvurusuna](https://code.visualstudio.com/docs/agents/reference/mcp-configuration)
bak. Aşağıdaki örnek, üst düzey anahtarı `servers` olan, editörün yönettiği biçimi kullanır.
Çalışma alanı dosyası `.vscode/mcp.json` ya da **MCP: Open User Configuration** komutuyla
açılan kullanıcı profili dosyası (`%APPDATA%\Code\User\mcp.json`,
`~/Library/Application Support/Code/User/mcp.json` ya da `~/.config/Code/User/mcp.json`):

```json
{
  "servers": {
    "praxi": {
      "type": "stdio",
      "command": "bot",
      "args": ["mcp", "serve", "--bot", "<BOT-UUID>", "--token-id", "<CREDENTIAL-ID>"]
    }
  }
}
```

`BOT_SECRETS_PASSPHRASE` vermek zorundaysan VS Code'un `inputs` düzenini kullan. Böylece parola
dosyada durmaz, sana sorulur ve VS Code'un gizli bilgi deposunda saklanır:

```json
{
  "inputs": [
    { "type": "promptString", "id": "praxi-pass", "description": "Praxi store passphrase", "password": true }
  ],
  "servers": {
    "praxi": {
      "type": "stdio",
      "command": "bot",
      "args": ["--secrets-backend", "file", "mcp", "serve", "--bot", "<BOT-UUID>", "--token-id", "<CREDENTIAL-ID>"],
      "env": { "BOT_SECRETS_PASSPHRASE": "${input:praxi-pass}" }
    }
  }
}
```

Sunucuyu kaydın üstündeki “Start” kod merceğinden başlat, sonra agent kipini kullan. Bu tarif,
editörün yönettiği MCP içindir. Daha yeni Copilot altyapısı taşınabilir `mcpServers`
yapılandırmasını kullanır. VS Code girdi değişkenleri editör biçimine özgüdür ve altyapı
oturumlarına aktarılmaz.

### Zed

Resmî [Zed MCP rehberine](https://zed.dev/docs/ai/mcp) bak. Zed bunlara context server der
ve ayar dosyasında tutar (macOS ve Linux'ta `~/.config/zed/settings.json`, Windows'ta
`%APPDATA%\Zed\settings.json`; `zed: open settings file` komutuyla aç):

```json
{
  "context_servers": {
    "praxi": {
      "command": "bot",
      "args": ["mcp", "serve", "--bot", "<BOT-UUID>", "--token-id", "<CREDENTIAL-ID>"],
      "env": {}
    }
  }
}
```

Sunucu başlatılınca Agent paneli ayarlarında yeşil bir nokta görünür.

### Genel stdio istemcisi

Bir komut başlatabilen her istemci çalışır: komut `bot`, bağımsız değişkenler yukarıdaki gibi,
ağ yok, ek ortam yok. İstemci olmadan hızlı deneme:

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  | bot mcp serve --bot <BOT-UUID> --token-id <CREDENTIAL-ID>
```

### Doğrulama durumu

Bu örnekler, yukarıda bağlantısı verilen resmî yapılandırma sayfalarını izler ve 2026-11-22
tarihinde kontrol edilmiştir. **Kurulu istemcilerde doğrulanmamıştır**: her istemcinin kurulu
bir kopyasında çalıştırılmadılar, bu yüzden hiçbir istemci sürümünün doğrulandığı iddia
edilmez. CI'da test edilen şey, her istemcinin dayandığı sunucu tarafıdır.
`apps/cli/src/mcp_serve/compat_tests.rs` ham JSON-RPC çerçevelerini sürer (protokol sürümü
anlaşması, yetenekler, `tools/list` ve imleçler, `isError` içeren `tools/call` sonuçları,
`ping`, bilinmeyen yöntemler, toplu istekler, geçersiz JSON, aşırı büyük çerçeveler ve
`notifications/cancelled`). `apps/cli/tests/mcp_server_cancellation.rs` ise gerçek süreci,
sahte bir sağlayıcıyla gerçek bir loopback daemonuna karşı sürer: gerçek araç kataloğu ve
açıklamaları, görev başlatma/durum/sonuç/iptal, büyük ama geçerli girdi, sınırı aşan girdinin
reddi, canlı süre dolumu ve iptal, hız sınırı ve bekleyen bir bilgisayar onayı sırasında
iptal. Yalıtılmış testler şifreli dosya tabanlı bir kimlik bilgisi deposu kullanır. İstemcinin
başlatma ortamlarından işletim sistemi anahtarlığına erişimi doğrulamazlar. Belirli bir
istemci sürümünde bir tarifi doğrularsan lütfen buraya ekle.

## Görev araçları

Varsayılan katman `--allow tasks`'tır ve `mcp:tasks` kapsamını gerektirir.

| Araç | Bağımsız değişkenler | Sonuç | Salt okunur etiketi |
| --- | --- | --- | --- |
| `bot_list` | `{}` | Seçilen botun kimliği ve adı | Evet |
| `task_start` | `{ "text": "...", "bot": "optional selected UUID" }` | Yeni çalışma kimliği | Hayır |
| `task_status` | `{ "run": "UUID" }` | Durum ve dış ajan bilgisi | Evet |
| `task_result` | `{ "run": "UUID" }` | Hazır olup olmadığı, sınırlı ve gizli bilgileri maskelenmiş metin ve hata işareti | Evet |
| `task_cancel` | `{ "run": "UUID" }` | İptal onayı | Hayır |
| `schedule_list` | `{}` | Bu botun zamanlama kimlikleri, adları ve açık/kapalı durumları | Evet |

Her görev kendi yeni sohbetinde başlar. Görev başlatma hemen döner. Bot çalışırken ya da onay
beklerken durum ve sonuç araçlarını kullan. Bir kimlik bilgisi yalnızca seçilen bot için
kendi oluşturduğu çalışmaları inceleyebilir ve iptal edebilir. Zamanlama istemleri ve rastgele
sohbet geçmişi bu araçlarla açılmaz.

Bot, kendi yapılandırılmış izinlerini kullanır. Dış görev metni `external_agent` rolüyle
saklanır, sonra modele açıkça güvenilmeyen bir veri bloğu olarak sunulur. Bot politikasını
geçersiz kılamaz. Otomatik hafıza çağırma ve önbellekteki komut/iş akışı yönlendirmesi atlanır.
Görev sohbeti yeni mesaj kabul edemez ve sonradan insan sohbetine dönüşemez. Bunun yerine ayrı
bir görev ya da sohbet başlat.

## Bilgisayar araçları

Bilgisayar erişimi hem kimlik bilgisi kapsamını hem de açıkça seçilmiş bir sunucu katmanını
gerektirir:

```sh
bot token create --scope mcp:tasks,mcp:computer --bot 00000000-0000-0000-0000-000000000001 --ttl 1d
bot mcp serve --bot 00000000-0000-0000-0000-000000000001 --allow tasks,computer --token-id 00000000-0000-0000-0000-000000000002
```

`--allow computer`, yalnızca bilgisayar kapsamlı bir kimlik bilgisi için de çalışır. Sunucu,
seçilen botun kullanabildiği `browser_*` ve `desktop_*` araçlarını mevcut kayıt defterinden
listeler. `browser_eval` ve kabuk araçları bu katman tarafından asla dışarı açılmaz.
Dışarı açılan adlar tek tek gözden geçirilmiştir. Yeni bir yerel bilgisayar aracı eklemek,
ileride eklenecek toplu eylem araçları da dahil, bu izin listesinin güncellenmesini gerektirir.

Her bilgisayar çağrısı, diğer Praxi çağrılarıyla aynı izin motorunu, onay kartlarını,
bilgisayar hedefini, kontrol kapısını, zaman aşımını ve denetim yolunu kullanan, kaynağı
belli normal bir araç çalışması başlatır. MCP isteği, tamamlanmayı ya da onayı en fazla on
dakika bekler, sonra çalışmayı iptal eder ve hata döndürür. Bir ret de MCP hata sonucu olur.
Hiçbir MCP işlemi bir eylemi onaylayamaz, kontrolü bırakamaz, politikayı ya da botun
yapılandırmasını değiştiremez.

## Onaylar ve yetki

Dış ajanın yaptığı her araç çağrısı, okumalar ve Meta araçlar dahil, **Bir kez izin ver**
ister. Mevcut izinler, okuma kısayolu ve tam yetkili bir bot politikası bu tabanı kaldıramaz.
Kapı, botun mevcut Human politikasını ve ExternalAgent politikasını birlikte değerlendirir.
Birinden gelen ret kazanır. Açık bir ExternalAgent izni Human retini gevşetemez, bir Human izni
de temel ya da ExternalAgent retini aşamaz. İkisi de çağrıya izin verdiğinde bile çağrı yine
**Bir kez izin ver** ister. Onayı Praxi'nin mevcut arayüzünde, CLI'da ya da Telegram onay
akışında ver. **Her zaman**, kategori için her zaman ve **Bu sohbet** gibi kalıcı kapsamlar bu
başlatıcı için reddedilir.

Bu, sıradan Praxi çalışmalarından bilerek daha katıdır. Bağımsız yerel çıkış/taint çalışması
bu özelliğin temelinde yoktur, bu yüzden bu sürüm o ayrı politikayı uyguladığını iddia etmez.
Bunun yerine kalıcı bir dış kaynak işareti ve çağrı başına onay kullanır. Devam ettirilen
çalışmalar da zaten saklanmış alt çalışmalar da bu kaynak işaretini korur. Saklanmış bir dış
alt çalışmanın görevi ve üst bağlamı da sabit “güvenilmeyen kullanıcı verisi” sarmalayıcısını
alır. Yeni alt ajan başlatma reddedilir. Böylece kimlik bilgisinin çalışma sınırı, çoğalan
alt çalışmalarla aşılamaz. Yedek modeller ve alt model rolleri dahil yerel `cli:` model
oturumları reddedilir, çünkü bunlar araçları ana makinenin çağrı başına kapısının dışında
çalıştırabilir. MCP görevleri için API tabanlı bir model kullan.

Bunlar uygulama izinleridir. Aynı işletim sistemi kullanıcısı olarak çalışan bir program bu
sınırın dışındadır ve dosya sistemine ya da anahtarlığa zaten erişebilir.

## Sınırlar, iptal ve depolama

Her kimlik bilgisi, kayan bir dakika içinde 30 araç çağrısına ve iki etkin çalışmaya izin verir.
Duraklatılmış çalışmalar ve saklanmış alt çalışmalar da buna dahildir. Sahibin oluşturduğu ayrı
bir kimlik bilgisinin kendi sınırları vardır. Hız penceresi bellektedir ve daemon yeniden
başlayınca sıfırlanır. Etkin çalışma kabulü, saklanan çalışmalardan yeniden kurulur. Daemon
yükleri 64 KiB JSON ile sınırlıdır ve üretimdeki maskeleyiciden geçer. MCP kendi sınırlı metin
sarmalayıcısını ve protokol çerçevelemesini ekler. Sonuçlar güvenilmeyen veri olarak
etiketlenir. Girdiler 32 KiB ile sınırlıdır, JSON-RPC çözümlemesinden önce de 64 KiB'lik bir MCP
çerçeve sınırı vardır.

Kimlik bilgisi ömürleri 60 saniye ile 30 gün arasındadır. Süre dolumu her kapı isteğinde ve
bilgisayar çağrıları beklerken denetlenir. Bir kimlik bilgisini herkese açık kimliğiyle iptal et:

```sh
bot token revoke 00000000-0000-0000-0000-000000000002
```

İptal, bekleyen görevleri ve alt çalışmalarını durdurmadan önce yetkiyi geçersiz kılar. Zaten
onaylanıp tamamlanmış eylemleri geri almaz. Kapsamlı token'lar yalnızca `GET /v1/mcp-server/tools`
ve `POST /v1/mcp-server/call` yollarını kullanabilir. Sahip politikası, gizli bilgi, sağlayıcı,
MCP yapılandırması ve onay uç noktaları onları reddeder. Eşleştirilmiş her eş rolü, dört MCP
kapısı ve kimlik bilgisi yönetimi yolunun hepsinde reddedilir.

Ayrılmış `mcp-server/` gizli bilgi deposu alanı bearer değerini ve yetkiyi içerir. Windows
kimlik bilgisi deposunun aynı ad saydığı takma adlar dahil, büyük/küçük harfin her ASCII yazımı
tüm platformlarda ayrılmıştır. Bu alan bir sağlayıcı ya da bağlayıcı gizli bilgisi olarak
seçilemez, genel gizli bilgi komutlarıyla değiştirilemez, bir durum paketine aktarılamaz ve
bir paketten içe aktarılamaz. Taşınan bir kurulum için yeniden MCP kimlik bilgisi verilmesi
gerekir. Denetim kayıtları yalnızca işlem etiketlerini ve herkese açık kimlikleri içerir.
Denetim yazımı başarısız olursa eylem engellenir.

## Sorun giderme

`no matching scoped token; run bot token create first`, bu bota ve istenen katmanlara izin veren
canlı bir kimlik bilgisi olmadığı anlamına gelir. Sahip olarak uyan bir kimlik bilgisi oluştur
ya da onun kimliğini seç. `multiple matching scoped tokens; specify --token-id` mesajı bu
bayrağı ister. Süresi dolmuş ya da iptal edilmiş bir kimlik bilgisi yeniden bağlanamaz.

Bir çağrı bekliyorsa onayı Praxi'de incele. Bir bot yerel bir CLI modeli kullanıyorsa API
tabanlı bir model seç. İki çalışma sınırına ulaşıldıysa mevcut görevleri tamamla ya da iptal et.
429 yanıtı bir MCP araç hatası olarak döner. Yeniden denemeden önce kayan hız penceresinin
dolmasını bekle.

### İstemcide sık görülen hatalar

| Mesaj ya da belirti | Neden ve çözüm |
| --- | --- |
| `cannot unlock the scoped credential store without an interactive prompt` | Süreç gizli bilgi deposuna ulaşamıyor: işletim sistemi anahtarlığı kilitli ya da istemcinin başlatma ortamında kullanılamıyor (SSH oturumu, servis, yalıtılmış başlatıcı) ya da depo şifreli bir dosya ve `BOT_SECRETS_PASSPHRASE` ayarlı değil. İstemciyi normal bir masaüstü oturumundan başlat ya da `--secrets-backend file` ile dosya arka ucunu seçip parolayı istemcinin ortamı ya da girdi düzeniyle ver. |
| `bot token create` hemen ardından `no matching scoped token; run bot token create first` | Sunucu, kimlik bilgisini oluşturan klasörden farklı bir veri klasörüne baktı. Her iki komuta da aynı `--data-dir` değerini (`mcp`'den önce) ver ya da ikisi için de `BOT_DATA_DIR` ayarla. |
| Sunucu başarısız görünüyor ya da `MCP stdio initialization failed` | İstemci `bot` komutunu başlatamadı (mutlak yol kullan) ya da daemon beklenen kapıda çalışmıyor (`--port`, `--url`). Tam komutu elle çalıştır ve stderr çıktısını oku. |
| Çalışan bir bağlantıdan sonra `MCP daemon refused the request (401)` | Kapsamlı kimlik bilgisinin süresi dolmuş ya da iptal edilmiş olabilir. Uyan bir kimlik bilgisi oluştur ve `--token-id` değerini güncelle. Sunucu, daemon hata ayrıntılarını bilerek gizler. |
| `MCP daemon refused the request (403)` | Kimlik bilgisi seçilen bota, katmana ya da işleme izin vermiyor. Uyan kapsamlı bir kimlik bilgisi oluştur ya da izin verilen bir işlem seç. |
| `MCP daemon refused the request (429)` | Kimlik bilgisi kayan çağrı sınırına ya da etkin çalışma sınırına ulaştı. Hız penceresini bekle ya da mevcut bir çalışmayı bitir veya iptal et. |
| Bir çağrı takılıyor, sonra istemci zaman aşımı bildiriyor | Çağrı, Praxi'de **Bir kez izin ver** onayını bekliyor. Onayı orada ver ya da istemcinin araç zaman aşımını uzat (Claude Code: `MCP_TOOL_TIMEOUT`). İstemci tarafındaki bir iptal, çağrıyı ve bekleyen onayını durdurur. |
| `cursor` içeren `tools/list` çağrısı `-32602` ile başarısız oluyor | Araç listesi tek sayfadır ve sunucu hiç imleç vermez. İmleç gönderme. |
| İstemci JSON-RPC toplu istekleri gönderiyor | Toplu istekler MCP 2025-06-18 sürümünde kaldırıldı ve `-32600` ile reddedilir. İstemciyi güncelle. |
| Çok büyük bir istekte bağlantı kapanıyor | 64 KiB'yi aşan tek bir JSON-RPC satırı aktarımı kapatır. Görev metnini küçük tut (araç girdileri 32 KiB ile sınırlıdır). |
