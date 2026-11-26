# Kurulum ve ilk çalıştırma

Praxi'yi edinmenin iki yolu var: Windows yükleyicisi (terminal gerekmez) ya da kaynaktan derleme (CLI ve daemon için her işletim sistemi). Şimdiye kadarki tek sürüm, ön sürüm `v0.1.0-alpha.1` (bir Windows yükleyicisi). Yani başka durumlarda yükleyiciyi kendin derlersin ya da CLI'yı derlersin.

## A seçeneği: Windows masaüstü uygulaması

Yükleyici, yalnızca senin hesabın için kurulan tek bir NSIS `.exe` dosyasıdır. İçinde masaüstü kabuğu (`bot-desktop.exe`) ve daemon (`bot.exe`, yardımcı servis olarak kullanılır) bulunur.

1. Praxi-Bot kurulum `.exe` dosyasını çalıştır. `%LOCALAPPDATA%\Programs\Praxi-Bot` altına kurulur ve yönetici hakkı gerektirmez.
2. Derlemeler kod imzalanana kadar Windows SmartScreen seni uyarabilir. **Daha fazla bilgi**'yi, sonra **Yine de çalıştır**'ı seç.
3. İlk açılışta uygulama paketle gelen daemon'ı başlatır ve beş adımlı bir sihirbaz açar:
   1. **Hoş geldin**: arayüz dili (İngilizce ya da Türkçe) ve tema (açık, koyu, sistem). Praxi önce yereldir, telemetri yoktur.
   2. **Sağlayıcı**: Google Gemini, Anthropic, OpenAI uyumlu bir servis ya da yerel Ollama'yı seç, API anahtarını yapıştır ve **Bağlantıyı dene**'ye bas. Anahtar doğrudan Windows Kimlik Bilgileri Yöneticisi'ne gider ve bir daha gösterilmez. Google Gemini'nin Google AI Studio üzerinden, kredi kartı istemeyen ücretsiz bir API katmanı var.
   3. **Güvenlik**: varsayılan izin hazır ayarını seç. Varsayılan **Yıkıcı işlemlerde onay**'dır ([Sohbet ve onaylar](chatting-and-approvals.md) sayfasına bak): okumak serbesttir, dosyaları değiştiren, masaüstünde tıklayıp yazan, komut çalıştıran ya da mesaj gönderen her şey önce sorar. Para harcayan işlemler her zaman sorar, kritik kimlik bilgisi ve sistem yolları her zaman engellenir, `Ctrl+Alt+Esc` botu istediğin an durdurur.
   4. **İlk botun**: bir ad, bir avatar ve renk, ayrıca seçtiğin sağlayıcıya göre doldurulan model rolleri (`executor`, `planner`, `classifier`).
   5. **Canlı dene**: *“Ekranımda ne var?”* ya da *“Not Defteri'ni aç ve merhaba yaz”* düğmesine tıkla, botun yanıtını izle ve canlı bir izin kartını gerçek zamanlı onayla ya da reddet.
4. Sihirbazı her adımda atlayabilir, sonra **Ayarlar, İlk kurulum sihirbazı** üzerinden yeniden çalıştırabilirsin.

Windows oturumu açılınca otomatik başlatma, Ayarlar'da *Windows oturumu açılınca başlat* seçeneğini açmadıkça (ya da yükleyiciye `/AUTOSTART=1` vermedikçe) **kapalıdır**. Kaldırınca, **Uygulama verilerini de sil**'i işaretlemedikçe sohbetlerin, botların ve ayarların korunur. Ayrıntı için [Windows'a kurulum](install-windows.md) sayfasına bak.

Uygulamanın genel acil durdurma kısayolu `Ctrl+Alt+Esc`'tir (uygulamada değiştirilebilir).

Masaüstü denetimi (başka uygulamalarda tıklama ve yazma) yalnızca Windows için gerçekleştirildi. Diğer sistemlerde daemon çalışır ama masaüstü araçları olmaz.

## Masaüstü bağlantısı desteği

Windows yükleyicisi, hesabın için `praxi://` bağlantılarını kaydeder. Birini açarsan masaüstü uygulaması başlar ya da var olan penceresi öne gelir. Kaldırınca o kuruluma ait bağlantı işleyicisi de silinir. Desteklenen hedefler için [Praxi bağlantılarını açma](chatting-and-approvals.md) bölümüne bak.

Linux ve macOS için masaüstü paket entegrasyonu henüz yapılmadı. Linux'ta kurulu bir masaüstü kaydı olmayan taşınabilir derleme, `praxi://` bağlantılarını işleyemez. macOS'ta kayıt, kurulu uygulama paketinden gelir.

## B seçeneği: CLI'yı kaynaktan derle

Gerekenler: Rust 1.89 ya da daha yenisi, bir C araç zinciri (Windows'ta MSVC derleme araçları), masaüstü uygulaması için ayrıca Node 24+ ve pnpm 10+.

```sh
cargo build --release -p cli      # produces target/release/bot
./target/release/bot --version
```

Masaüstü uygulamasını geliştirme kipinde, belirleyici sahte sağlayıcıyla (API anahtarı gerekmez) çalıştırmak için `PRAXIBOT_FAKE_PROVIDER=1 pnpm dev:desktop` kullan. Windows yükleyicisini kendin derlemek için aşağıdaki “Windows yükleyicisini kendin derle” bölümüne bak.

## Windows yükleyicisini kendin derle

Tauri, `bot` CLI ikili dosyasını [`apps/desktop/tauri.conf.json`](../../../apps/desktop/tauri.conf.json) içindeki `bundle.externalBin: ["binaries/bot"]` ile paketler. Tauri, `tauri build` çalışmadan önce yardımcı ikili dosyanın adının Rust hedef üçlüsüyle (`rustc -Vv | grep host`) `apps/desktop/binaries/` içinde bulunmasını bekler:

```powershell
# 1. Determine target triple (e.g. x86_64-pc-windows-msvc on 64-bit Windows MSVC)
$triple = (rustc -Vv | Select-String "host:").ToString().Split(":")[1].Trim()

# 2. Build the CLI daemon in release mode
cargo build --release -p cli

# 3. Stage the sidecar binary with the target-triple suffix expected by Tauri externalBin
New-Item -ItemType Directory -Force -Path apps/desktop/binaries | Out-Null
Copy-Item "target/release/bot.exe" "apps/desktop/binaries/bot-$triple.exe" -Force

# 4. Build the desktop UI and package the per-user NSIS installer
pnpm build:desktop
```

Ortaya çıkan yükleyici `target/release/bundle/nsis/Praxi-Bot_<version>_x64-setup.exe` yoluna yazılır. Çalışma sırasında [`apps/desktop/src/daemon.rs`](../../../apps/desktop/src/daemon.rs) daemon ikili dosyasını şu öncelik sırasıyla bulur:

1. `PRAXIBOT_BOT_BIN` ortam değişkeni (tanımlıysa)
2. `bot-desktop.exe`'nin yanındaki paketle gelen yardımcı dosya (`bot-<target-triple>.exe` ya da `bot.exe`)
3. `apps/desktop/binaries/bot-<target-triple>.exe` içindeki hazırlanmış yardımcı dosya
4. `PATH` üzerindeki `bot`

Kod imzalama alanları (`certificateThumbprint`, `digestAlgorithm`, `timestampUrl`), [`apps/desktop/tauri.conf.json`](../../../apps/desktop/tauri.conf.json) içinde `bundle.windows` altında yer tutucu olarak önceden ayarlanmıştır.

## Terminalden ilk çalıştırma

```sh
bot setup          # interactive: provider, key, first bot, permission preset
bot serve          # start the daemon (leave it running)
```

İkinci bir terminalde:

```sh
bot status                      # daemon health, pending approvals
bot bots list                   # shows your bots and their ids
bot chat <bot-id> "What files are in my Documents folder?"
```

`bot setup` sırayla şunları sorar: mesajların dili, sağlayıcı, API anahtarı (gizli girdi), model, ilk bot hazır ayarı (`general`, `browser` ya da `desktop`), bot adı ve varsayılan izin hazır ayarı. Sihirbaz bitene kadar hiçbir şey kaydedilmez. Anahtar, düz metin bir dosyaya değil gizli bilgi deposuna kaydedilir.

Betikle kurulum (örneğin CI'da ya da bir dotfiles betiğinde):

```sh
export GEMINI_API_KEY=...        # your key
bot setup --provider gemini --key-env GEMINI_API_KEY --yes \
          --bot-preset general --permission approve-destructive
```

`--yes`, `--provider` ile birlikte zorunludur. Anlamı “asla sorma”dır.

### `bot serve` ne yapar

- `127.0.0.1:47821` adresini dinler (`--port` ya da `BOT_PORT` ile değiştirilir). Başka hiçbir ağ arayüzüne asla bağlanmaz.
- İlk başlangıçta `<data dir>/api-token` içinde rastgele 256 bitlik bir API belirteci oluşturur ve yolunu yazdırır. CLI ile masaüstü uygulaması onu oradan okur.
- Önceki bir kapanışta yarım kalan her çalışmayı duraklatır. Onu API üzerinden sürdür (`POST /v1/runs/{id}/resume`), bunun için henüz bir CLI komutu yok.
- Zamanlayıcıyı ve yapılandırılmışsa Telegram kanalını başlatır.
- Ctrl-C ile düzgünce kapanır: çalışan araç adımlarına bitmeleri için `--shutdown-grace` saniye (varsayılan 10) verilir, sonra bütün canlı çalışmalar bir sonraki açılış için duraklatılır.

`bot serve` için en az bir kullanılabilir sağlayıcı gerekir. Birini `bot setup` ya da `bot providers add` ile ekle. Daemon çalışırken eklenen sağlayıcı `POST /v1/providers/reload` sonrasında devreye girer (masaüstü uygulaması bunu senin yerine yapar).

### Dosyaların nerede durur

Veri klasörünün içinde:

| Dosya ya da klasör | İçeriği |
| --- | --- |
| `providers.toml` | Hangi sağlayıcıların açık olduğu, varsayılan model, her anahtarın nerede saklandığı |
| `api-token` | Yerel API belirteci (parola gibi davran) |
| `skills/` | Becerilerin, bkz. [Beceriler](skills.md) |
| `workflows/` | Kayıtlı iş akışları, bkz. [İş akışları](../workflows.md) |
| `channels.toml`, `telegram_state.json` | Telegram açma/kapama anahtarı ve eşleşmiş kullanıcılar |
| SQLite veritabanı ve çalışma başına olay günlükleri | Botlar, sohbetler, mesajlar, çalışmalar, onaylar, denetim günlüğü |

## Ortam değişkenleri

| Değişken | Etkisi |
| --- | --- |
| `BOT_DATA_DIR` | Veri klasörü |
| `BOT_PORT`, `BOT_URL` | Daemon portu ya da istemci komutları için tam daemon adresi |
| `BOT_CATALOG_DIR` | Yerleşik küme yerine sağlayıcı katalog `*.toml` dosyalarının klasörü |
| `BOT_SECRETS_BACKEND` | `auto` (varsayılan), `keyring` ya da `file` |
| `BOT_SECRETS_PASSPHRASE` | Şifreli dosya gizli bilgi deposu için parola (aksi halde sorulur) |
| `BOT_DESKTOP=off` | Daemon'ı masaüstü araçları olmadan başlat |
| `BOT_ALLOWED_ORIGINS` | API'yi çağırabilen ek tarayıcı kaynakları (virgülle ayrılır; bir arayüz geliştirmek için) |
