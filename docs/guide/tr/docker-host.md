# Praxi'yi Docker'da çalıştırma (7/24 açık sunucu)

Bu rehber, [`packaging/docker/Dockerfile`](../../../packaging/docker/Dockerfile) ve
[`packaging/docker/docker-compose.yml`](../../../packaging/docker/docker-compose.yml) dosyalarını
kullanarak Praxi'nin her an açık sunucusunu (`bot serve --host`) bir Linux konteynerinin içinde çalıştırır.

> **Durum.** Docker kurulumu `bot` programını çok aşamalı bir imajda kaynaktan derler ve
> `bot serve --host` komutunu yetkisiz `praxi` kullanıcısı olarak çalıştırır; süreçleri `tini`
> toplar ve `/v1/health` sağlık denetimi vardır. Systemd'li bir Linux sanal makinesinde asıl kurulum
> yolu, [Praxi'yi uzak sunucuda çalıştırma](remote-host.md) sayfasındaki imzalı kurulum programıdır.

## İmaj çeşitleri

[`packaging/docker/Dockerfile`](../../../packaging/docker/Dockerfile) aynı çok aşamalı derlemeden iki
çeşit üretir:

- **Temel başsız daemon (`WITH_DESKTOP=0`, varsayılan)**: Yalnızca çalışma zamanı kütüphanelerini
  (`libdbus-1-3`, `libssl3`, `ca-certificates`, `curl`, `tini`) kurar ve `bot serve --host`
  komutunu, `/var/lib/praxi` bir birim (volume) olarak, çalıştırır. Sanal ekran ya da tarayıcı olmadan
  sohbet, Telegram, zamanlamalar, MCP sunucuları ve web araçları için her an açık bir sunucu
  istediğinde bunu kullan.
- **Masaüstü ve tarayıcı çeşidi (`WITH_DESKTOP=1`)**: `Xvfb` (`:99`, `1920x1080x24`,
  `-nolisten tcp`, özel `0600` Xauthority), asgari bir Xfce oturumu (`xfwm4`, `xfce4-panel`,
  `xfdesktop`, özel D-Bus oturum yolu, AT-SPI2) ve `scripts/install-linux.sh` betiğinin kullandığı
  aynı sabitlenmiş **Google Chrome for Testing `157.0.8103.38`** ikili dosyasını
  (`/opt/praxi-browser/chrome`, SHA-256 doğrulamalı) ekler.

İki çeşit de yetkisiz `praxi` kullanıcısı olarak çalışır (`UID 10001`, `GID 10001`) ve imaja
**hiçbir gizli bilgi** gömmez.

## İlk çalıştırma

1. [`packaging/docker/.env.example`](../../../packaging/docker/.env.example) dosyasından **ortam
   dosyasını oluştur** ve `/var/lib/praxi/secrets.enc` için rastgele bir parola üret:

   ```sh
   cp packaging/docker/.env.example packaging/docker/.env
   # BOT_SECRETS_PASSPHRASE değerini güçlü, rastgele bir değerle değiştir, örneğin:
   openssl rand -hex 32
   ```

   Konteynerlerde işletim sistemi anahtarlığı yoktur, bu yüzden `BOT_SECRETS_BACKEND=file`
   için `BOT_SECRETS_PASSPHRASE` gerekir (ya da bağlanmış bir Docker secret dosyasını gösteren
   `BOT_SECRETS_PASSPHRASE_FILE`). `.env` dosyasını git'ten ve paylaşılan disklerden uzak tut.

2. Docker Compose ile **konteyneri derle ve başlat**:

   ```sh
   docker compose -f packaging/docker/docker-compose.yml up -d --build
   ```

   Bunun yerine masaüstü ve tarayıcı çeşidini derlemek için `docker-compose.yml` içinde
   `WITH_DESKTOP: "1"` yaz (ve aşağıdaki `--security-opt` notlarına göz at) ya da doğrudan derle:

   ```sh
   docker build --build-arg WITH_DESKTOP=1 -f packaging/docker/Dockerfile -t praxi-host:desktop .
   ```

3. **Sağlığı ve günlükleri denetle**:

   ```sh
   docker compose -f packaging/docker/docker-compose.yml ps
   docker compose -f packaging/docker/docker-compose.yml logs -f praxi
   docker exec praxi-host curl -fsS http://127.0.0.1:47821/v1/health
   ```

   Yeni bir konteyner, bir model sağlayıcısı ayarlanana kadar HTTP 200 ile
   `"status":"setup_required"`, sonra `"status":"ok"` yanıtı verir.

4. **Bir sağlayıcı, bir bot ve isteğe bağlı Telegram kanalını ayarla**:

   ```sh
   docker exec -it praxi-host bot setup
   docker exec -it praxi-host bot doctor
   docker exec -it praxi-host bot channels telegram setup   # isteğe bağlı
   docker exec -it praxi-host bot channels telegram pair    # isteğe bağlı
   ```

   `.env` dosyasında sağlayıcı API anahtarı ortam değişkenlerini (`ANTHROPIC_API_KEY` ya da
   `GEMINI_API_KEY` gibi) ayarlamış olsan bile varsayılan sağlayıcı ve ilk bot ayarlansın diye
   `bot setup` (ya da `bot providers add`) komutunu yine de çalıştır.

## Masaüstü uygulamasını konteyner sunucusuyla eşleştirme

Praxi masaüstü uygulaması, konteyner sunucusuna dışa doğru açılan bir iroh eş kanalı (röle yedekli
QUIC) üzerinden bağlanır. Sunucunun güvenlik duvarında ya da yönlendiricide `47821` portunu açman
**gerekmez**.

1. Konteynerin sunucu modunda çalıştığını (`bot serve --host`, imajın varsayılan `CMD` komutudur) ve
   röleler için giden HTTPS/UDP çıkışının olduğunu doğrula.
2. Konteynerin içinde tek kullanımlık bir eşleştirme bileti üret:

   ```sh
   docker exec -it praxi-host bot host invite --role desktop
   ```

3. Praxi masaüstü uygulamasında üstteki bilgisayar seçiciyi aç, **Sunucu ekle** seçeneğini seç ve
   bileti yapıştır (biletler iki dakika sonra geçersiz olur ve bir kez işe yarar).

`docker exec`, `/var/lib/praxi` içinde `praxi` olarak çalıştığı için `bot host invite`
`/var/lib/praxi/owner-key` dosyasını okur ve **yönetici masaüstü** daveti verir (systemd sunucularındaki
`sudo praxi host invite --role desktop` ile aynı). Bu yönetici masaüstü botları, sohbetleri,
çalışmaları ve canlı ekran devralmayı yönetebilir (`WITH_DESKTOP=1` ile derlendiyse). Gizli bilgiler,
yürütme politikası, MCP sunucuları, beceriler, e-posta ve web arama kimlik bilgileri ile Telegram
eşleştirmesi ise yerelde, konteynerin içinde yönetilir (`docker exec -it praxi-host bot ...`).

Eşleştirilmiş cihazları listelemek, onlara izin vermek ya da izni geri almak için:

```sh
docker exec praxi-host bot host devices
docker exec praxi-host bot host screen --all-desktops
docker exec praxi-host bot host screen <key> --view-only
docker exec praxi-host bot host screen <key> --revoke
```

## Yükseltmeler

1. **Önce birimi yedekle** (aşağıdaki Birimin yedekleri bölümüne bak).
   Daha yeni bir `bot` sürümünde SQLite veritabanını açmak onu ileri doğru taşıyabilir, bu yüzden
   imajı değiştirmeden önce yükseltme öncesi bir anlık yedek tut.
2. **Kaynak değişikliklerini çek ve yeniden derle**:

   ```sh
   git pull
   docker compose -f packaging/docker/docker-compose.yml build --pull
   ```

3. **Düzgünce durdur ve yeniden oluştur**:

   ```sh
   docker compose -f packaging/docker/docker-compose.yml stop
   docker compose -f packaging/docker/docker-compose.yml up -d
   ```

   Durdurma, `tini` üzerinden `SIGTERM` gönderir. Böylece `bot serve`, `--shutdown-grace` süresi
   içinde (varsayılan 10 saniye) canlı çalışmaları düzgünce duraklatır ve yeni konteyner başlamadan
   önce SQLite'ı kapatır.

4. **Doğrula**:

   ```sh
   docker exec praxi-host bot doctor
   ```

## Birimin yedekleri

Kalıcı sunucu durumunun tamamı `/var/lib/praxi` biriminde (`docker-compose.yml` içinde `praxi-data`)
durur: SQLite veritabanı, `secrets.enc`, `api-token`, `owner-key`, eş kimliği, `host.toml` ve
tarayıcı profilleri.

### Birimin tam anlık yedeği (yükseltmeden önce önerilir)

SQLite boşta olsun ve anlık yedek tutarlı çıksın diye önce konteyneri durdur:

```sh
docker compose -f packaging/docker/docker-compose.yml stop
docker run --rm \
  -v praxi-data:/data:ro \
  -v "$PWD":/backup \
  debian:bookworm-20261104-slim \
  tar -C /data -czf "/backup/praxi-data-$(date -u +%Y%m%dT%H%M%SZ).tgz" .
docker compose -f packaging/docker/docker-compose.yml start
```

> [!IMPORTANT]
> Birim arşivi `secrets.enc`, `api-token`, `owner-key` ve eş anahtarlarını içerir,
> `BOT_SECRETS_PASSPHRASE` parolasını **içermez**. `.env` dosyasındaki (ya da Docker secret
> dosyandaki) parolayı yedek arşivinden ayrı sakla. O olmadan `secrets.enc` çözülemez.

Bir birim anlık yedeğini temiz bir birime geri yüklemek için:

```sh
docker compose -f packaging/docker/docker-compose.yml stop
docker run --rm \
  -v praxi-data:/data \
  -v "$PWD":/backup:ro \
  debian:bookworm-20261104-slim \
  sh -c 'rm -rf /data/* /data/.[!.]* && tar -C /data -xzf /backup/praxi-data-YYYYMMDDTHHMMSSZ.tgz'
docker compose -f packaging/docker/docker-compose.yml up -d
```

### Taşınabilir durum yedeği (`bot backup` / `bot restore`)

Taşınabilir durumu (botlar, sohbetler, hafıza, beceriler, iş akışları, zamanlamalar, sağlayıcı
ayarları; API anahtarları, token'lar, eşleştirilmiş cihazlar ve tarayıcı profilleri hariç)
[Şifreli yedekleme ve geri yükleme](backup.md) sayfasındaki gibi dışa da aktarabilirsin.
Parolayı bir dosyada ya da Docker secret içinde tut, komut satırına asla yazma:

İmaj `BACKUP_PASSPHRASE` değişkenini tanımlamaz, onu komut için sen ayarlarsın
(`docker exec -e BACKUP_PASSPHRASE ...` değeri komut satırına yazmadan kabuğundan geçirir ya da
bağlanmış bir Docker secret ile `--passphrase-file` kullan). `--passphrase-env` değişkenin
**adını** alır, parolanın kendisini asla.

```sh
export BACKUP_PASSPHRASE='...'   # yalnızca senin kabuğunda; imajda saklanmaz
docker exec -e BACKUP_PASSPHRASE praxi-host bot backup create /tmp/praxi-backup.bak --passphrase-env BACKUP_PASSPHRASE
docker cp praxi-host:/tmp/praxi-backup.bak ./praxi-backup.bak
docker exec praxi-host rm -f /tmp/praxi-backup.bak
```

`bot backup restore`, `bot serve` dinlerken çalışmayı reddettiği için geri yüklemeyi daemon
durdurulmuşken, aynı birim üzerinde tek seferlik bir konteynerle yap, sonra servisi başlat ve gizli
bilgileri yeniden gir.

## Güvenlik notları

### Daemon portunu asla herkese açma

- `bot serve` kesin olarak `127.0.0.1:47821` adresine bağlanır (yalnızca loopback) ve `Host` başlığı
  loopback olmayan (`127.0.0.1`, `localhost`, `[::1]`) ya da `Origin` değeri izinli olmayan
  istekleri reddeder.
- `0.0.0.0:47821:47821` portunu asla yayımlama ve daemon API'sinin önüne herkese açık bir ters proxy
  koyma. Bir token ile API'ye ulaşan herkes botları yönetebilir ve sunucu durumunu okuyabilir.
- `docker-compose.yml` içinde port eşlemesi `127.0.0.1:47821:47821` olarak durur. Docker'ın varsayılan
  köprü ağında `bot serve` konteynerin `eth0` arayüzünde değil, konteynerin kendi `127.0.0.1`
  adresinde dinler. Yani köprüden iletilen ve `127.0.0.1:47821` adresine gelen sunucu bağlantıları da
  reddedilir. iroh ya da Telegram ile eşleştirirken köprü kipini koru ve yönetimi yerelde
  `docker exec -it praxi-host bot ...` ile yap. Linux Docker sunucusunun kendisinden yerel HTTP API
  erişimine özellikle ihtiyacın varsa `network_mode: host` kullan (yine yalnızca loopback) ve ona bir
  SSH tüneliyle ulaş (`ssh -L 47821:127.0.0.1:47821 user@server`).

### Sahip anahtarına ve parolaya kim ulaşabilir

Praxi, genel daemon bearer token'ını (`/var/lib/praxi/api-token`, kip `0600`) **sahip anahtarından**
(`/var/lib/praxi/owner-key`, kip `0600`) ayırır. Yalnızca sahibe ait işlemler (araç çağrılarını
onaylamak, izin politikasını değiştirmek, gizli bilgileri düzenlemek, sunucuları eşleştirmek ya da
yetkisini kaldırmak, yönetici davetleri oluşturmak) yalnızca `api-token` ile değil,
`owner-key` (`x-praxi-owner-key`) ile yapılır.

Konteynerin içinde:

- **`owner-key` dosyasını kim okuyabilir**: `praxi-host` konteynerine karşı `docker exec` çalıştırabilen
  herkes, Docker sunucusunda `root` yetkisi (ya da Docker soketi erişimi) olan herkes ve konteynerin
  içinde `praxi` kullanıcısı olarak çalışan her süreç (bir botun çalıştırdığı onaylı bir `shell_run`
  ya da `script_run` komutu dahil). Bu aynı kişiler `BOT_SECRETS_PASSPHRASE` için
  `/proc/<pid>/environ` dosyasına bakabilir ya da `/var/lib/praxi/secrets.enc` dosyasını okuyabilir.
- **Kim okuyamaz**: Kabuk ya da betik erişimi olmayan botlar, CLI kodlama motorları, MCP istemcileri ve
  yalnızca `api-token` taşıyan çağıranlara `owner-key` hiçbir zaman verilmez.
- **İşletme duruşu**: Docker daemon'una ya da `docker exec` erişimine sunucuda root yöneticiliği gibi
  davran, konteyneri ilgisiz kişisel veri bulunmayan ayrı bir sunucuda ya da sanal makinede tut ve
  bot izin hazır ayarını **Yıkıcı işlemlerde onay** ya da daha katı bir düzeyde tut
  (bak: [Güvenlik modeli](security.md)).

### `WITH_DESKTOP=1` için Chrome korumalı alanı (sandbox) ve `--security-opt`

`browser-cdp`, yetkisiz Linux süreçleri için `--no-sandbox` seçeneğini reddeder; böylece Chromium
kendi işleyici (renderer) korumalı alanını açık tutar. Linux'ta Chromium'un korumalı alanı yetkisiz kullanıcı
ad alanlarına (`CLONE_NEWUSER`) dayanır.

Systemd sunucularında `scripts/install-linux.sh`, `userns` iznini yalnızca
`/opt/praxi-browser/chrome` için veren, adı konmuş bir AppArmor profili
(`/etc/apparmor.d/praxi-browser`) kurar. Bir Docker konteynerinin içinde `docker build` çekirdek
AppArmor profillerini yükleyemez ve Docker'ın varsayılan `docker-default` AppArmor ile `seccomp`
profilleri konteynerin içinde yetkisiz kullanıcı ad alanlarını engeller. Ayrıca sunucu çekirdeğinde
`kernel.apparmor_restrict_unprivileged_userns=1` açık olan Ubuntu 24.04+ sunucularında, sunucu ya
konteyner için `userns` iznini veren özel bir AppArmor profili yüklemedikçe ya da bu sysctl'i
kapatmadıkça (`sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`) `--security-opt apparmor=unconfined`
bile kullanıcı ad alanlarını engeller.

Sonuç olarak tarayıcı otomasyonlu `WITH_DESKTOP=1` çeşidini çalıştırmak, genellikle `docker run` /
Compose çalışma zamanında konteyner güvenlik geçersiz kılmaları vermeyi gerektirir (Ubuntu 24.04+
sunucularında ayrıca `userns` izinli bir AppArmor profili ya da sysctl ayarı):

```sh
docker run -d \
  --security-opt seccomp=unconfined \
  --security-opt apparmor=unconfined \
  --env-file packaging/docker/.env \
  -v praxi-data:/var/lib/praxi \
  praxi-host:desktop
```

Paylaşımlı ya da çok kiracılı sunucularda `unconfined` yerine `userns` izni veren, dar kapsamlı özel
bir seccomp/AppArmor profilini tercih et ve `--privileged` ya da `--cap-add=SYS_ADMIN` kullanma.
Tarayıcı ya da X11 masaüstü otomasyonuna ihtiyacın yoksa varsayılan `WITH_DESKTOP=0` imajını kullan;
böylece X sunucusuna, masaüstü oturumuna ya da `--security-opt` gevşetmesine gerek kalmaz.
