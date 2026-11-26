# Windows indirmenin gerçek olduğunu denetle

İndirdiğin bir yükleyiciyi çalıştırmadan önce iki şeyi denetleyebilirsin:

1. **Dosya eksiksizdir ve projenin listelediği dosyadır.** (bir SHA-256 sağlama toplamı)
2. **GitHub, dosyanın Praxi-Bot sürüm iş akışı tarafından**, projenin kendi kaynak kodundan ve
   başka birinin bilgisayarında değil, derlendiğini doğrular. (bir derleme kökeni “attestation” kaydı)

İki denetim de bir dakika sürer. Programcı olman gerekmez ve denetim yükleyiciyi hiçbir zaman başlatmaz.

> **Henüz imzalı değil.** Yükleyicinin kod imzalama sertifikası yok, bu yüzden Windows SmartScreen
> yine “Windows bilgisayarınızı korudu” diyebilir (bkz. [Windows'a kurulum](install-windows.md)).
> Bu sayfadaki denetimler o uyarıyı kaldırmaz. Dosyanın gerçekten projenin kendi derlemesi olup olmadığını
> söylerler, SmartScreen'in bugün söyleyemediği şey budur.

## Her sürümde neler var

Bu sayfa eklendikten sonra derlenen her sürüm, bu dosyaları yükleyicinin yanında listeler. İhtiyacın
olanları aynı klasöre indir.

| Dosya | Nedir |
| --- | --- |
| `Praxi-...-setup.exe` | Windows yükleyicisi. |
| `SHA256SUMS.txt` | Aşağıdaki ve yukarıdaki her dosyanın beklenen SHA-256 değeri. |
| `praxi-bot-windows.sigstore.json` | Dosyaların nerede derlendiğinin imzalı kanıtı (attestation). |
| `verify-windows.ps1` | İki denetimi senin için yapan betik (isteğe bağlı). |
| `bot-x86_64-pc-windows-msvc.exe` | Yükleyicinin içindeki `bot` programı (daemon ve komut satırı). |
| `sbom-rust-cli.cdx.json`, `sbom-rust-desktop.cdx.json`, `sbom-ui.cdx.json` | Programın hangi kütüphanelerden oluştuğunu gösteren listeler (bkz. aşağıdaki “Parça listesi (SBOM)” bölümü). |
| `build-info.txt` | Dosyaların hangi commit, hangi araçlar ve hangi tarihle derlendiği. |

Bu özellik eklenmeden önce yayımlanan sürümlerde (en eski alfalarda) bu dosyaların hiçbiri yoktur.
Onlar için yalnızca dosyanın özetini (hash) kendin hesaplayıp projeden doğrulamasını isteyebilirsin.

## Kolay yol: tek betik

İndirdiğin dosyaların olduğu klasörde **PowerShell** aç ve şunu çalıştır (dosya adını ve etiketi
kendininkiyle değiştir; `-Tag` zorunludur ve olmadan betik açık bir iletiyle durur):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\verify-windows.ps1 `
  -InstallerPath .\Praxi-0.1.0-alpha.4-setup.exe -Tag v0.1.0-alpha.4
```

Her şey yolundaysa `Checksum: OK` ve `Provenance: OK` yazar. Bir şey yanlışsa
`NOT SAFE` der, durur ve yükleyiciyi çalıştırmamalısın. Kökeni denetlemek için
[GitHub CLI](https://cli.github.com) (`gh`, 2.80 ya da daha yeni sürüm) gerekir. O olmadan yalnızca
sağlama toplamını `-SkipProvenance` ile denetleyebilirsin. Betik o zaman kökenin denetlenmediğini açıkça söyler.

Betik depoda da var: [`scripts/release/verify-windows.ps1`](../../../scripts/release/verify-windows.ps1).
Yalnızca dosyaları okur ve `gh attestation verify` çalıştırır.

## Elle yol

### 1. Sağlama toplamı

```powershell
Get-FileHash .\Praxi-0.1.0-alpha.4-setup.exe -Algorithm SHA256
```

`SHA256SUMS.txt` dosyasını aç ve uzun onaltılık metni dosyanın satırıyla karşılaştır. Büyük ya da küçük
harf fark etmez. Her karakteri aynı olmalıdır. Aynı değilse indirme bozulmuştur ya da değiştirilmiştir:
sil ve yeniden indir.

Sağlama toplamı tek başına yalnızca bozuk bir indirmeye karşı korur. Bir sitedeki yükleyiciyi değiştirebilen
biri, yanındaki sağlama toplamını da değiştirebilir. İkinci denetimin bulunma nedeni budur.

### 2. Köken

[GitHub CLI](https://cli.github.com) kur (2.80 ya da daha yeni sürüm), sonra:

```powershell
gh attestation verify .\Praxi-0.1.0-alpha.4-setup.exe `
  --bundle .\praxi-bot-windows.sigstore.json `
  --repo emreocell/Praxi-Bot `
  --signer-workflow github.com/emreocell/Praxi-Bot/.github/workflows/installer.yml `
  --source-ref refs/tags/v0.1.0-alpha.4 `
  --deny-self-hosted-runners
```

`Verification succeeded` şu anlama gelir: GitHub'ın imzalama servisi, tam olarak bu dosyanın (denetim
SHA-256 üzerinden yapılır, dolayısıyla dosya adı önemli değildir) `emreocell/Praxi-Bot` deposunun
`installer.yml` iş akışı tarafından, GitHub'ın kendi bilgisayarlarında, senin belirttiğin etiketten
üretildiğini kaydetmiştir. Başka her durumda bir hata ve sıfırdan farklı bir çıkış kodu yazdırılır.

- **Paket dosyası olmadan:** `--bundle` satırını çıkar. O zaman `gh` doğrudan GitHub'a sorar ve bir kez
  `gh auth login` ile oturum açmanı ister.
- **Sürümün başka bir dosyası** (yan dosya, bir SBOM, `SHA256SUMS.txt`) aynı şekilde denetlenir:
  yolunu en başa koy ve diğer seçenekleri aynen bırak.
- **Bir etiketi değil, yalnızca projenin `main` üzerindeki en yeni derlemesini biliyorsan**
  `--source-ref refs/heads/main` kullan. Elinde bir etiket varsa onu kullan: daha sıkı denetim odur.

## Bilgisayarımdaki program kanıtlanmış olan mı?

Yükleyici `bot` programını `%LOCALAPPDATA%\Programs\Praxi-Bot` klasörüne koyar. Bu, sürümdeki
`bot-x86_64-pc-windows-msvc.exe` ile birebir aynı dosyadır (derleme bunu her çalışmada denetler),
yani karşılaştırabilirsin:

```powershell
Get-FileHash "$env:LOCALAPPDATA\Programs\Praxi-Bot\bot.exe" -Algorithm SHA256
```

Sonuç, `SHA256SUMS.txt` içindeki `bot-x86_64-pc-windows-msvc.exe` satırıyla eşleşmelidir. (Kullanıcı
başına bir kurulum varsayılır. “Herkes için” yapılan kurulum `C:\Program Files` altında bulunur.)

## Parça listesi (SBOM)

SBOM (“yazılım malzeme listesi”) programın parça listesidir: içine giren her kütüphane, sürümü ve
lisansıyla. Sürümde, güvenlik araçlarının okuyabildiği [CycloneDX](https://cyclonedx.org) biçiminde
üç tane vardır:

- `sbom-rust-cli.cdx.json`: `bot` programının (daemon ve komut satırı) neden oluştuğu.
- `sbom-rust-desktop.cdx.json`: masaüstü pencere kabuğunun neden oluştuğu.
- `sbom-ui.cdx.json`: uygulamanın ekranlarının çalışırken ihtiyaç duyduğu JavaScript paketleri.

Praxi'yi kullanmak için bunlara ihtiyacın yok. Önem verdiğin bir kütüphanenin ya da yeni bir güvenlik
sorunu bulunmuş bir kütüphanenin içinde olup olmadığını denetlemek isteyenler içindir.
[Grype](https://github.com/anchore/grype) ya da [OSV-Scanner](https://google.github.io/osv-scanner/)
gibi araçlar bu dosyaları doğrudan kabul eder. Praxi'nin nelerden derlendiğini listelerler, listedeki
hiçbir şeyin kusurlu olmadığına dair bir söz değillerdir. Yalnızca geliştirmede kullanılan araçlar
dışarıda bırakılır ve Rust bölümlerinin birkaç derleme zamanı bağımlılığı, son programın parçası
olmasalar bile listelenmiş olabilir.

## Bu denetimlerin sana söylemedikleri

- **Kaynak kodun zararsız olduğunu kanıtlamazlar.** Dosyanın projenin kendi deposundan, kendi iş akışıyla
  derlendiğini kanıtlarlar. Kod açık kaynaklıdır: oku ya da başkalarının okumasını bekle.
- **Yalnızca deponun ayarları kadar güçlüdürler.** Projede yazma erişimi olan biri bir daldan derlerse
  ya da iş akışını düzenlerse, köken o derleme için yine “geçerli” olur. Bu yüzden daha sıkı
  `--source-ref refs/tags/<tag>` biçimi önerilir ve son sürüm etiketleri Linux tarafında korunur,
  bkz. [Yayımlama](../../contributing/releasing.md).
- **Kod imzalama sertifikası değildirler.** Windows'un gösterebileceği bir yayımcı adı yoktur. Bir
  sertifika planlanıyor. O zamana kadar denetlemenin yolu bu sayfadır.

## Kendim yeniden derleyebilir miyim?

Aynı kaynağı derleyebilirsin, ama **aynı dosyayı bekleme**. Derlemeler bugün bit bit yeniden
üretilebilir *değildir* ve bu ikinci bir makinede denenmedi. Projenin sabitledikleri:

- tam kaynak commit'i (`build-info.txt` içinde ve kökenin kaydında adı geçer),
- Rust kütüphaneleri: `Cargo.lock`, `--locked` ile kullanılır, bu yüzden eski bir kilit dosyası derlemeyi bozar,
- JavaScript paketleri: `pnpm install --frozen-lockfile`,
- iş akışının kullandığı her GitHub Action, tam commit özetiyle,
- SBOM aracının (`cargo-cyclonedx`) sürümü.

Sabitlenmeyen ve baytları değiştirebilecek şeyler: Rust derleyicisi (depo `stable` kanalını ister,
yani sürüm kayar), Node ve pnpm yama sürümleri, `windows-latest` GitHub çalıştırıcı imajı ve Tauri'nin
indirdiği NSIS yükleyici araçları. `build-info.txt`, gerçekten kullanılan sürümleri ve kilit dosyası
özetlerini kaydeder, böylece bir yeniden derleme en azından birebir karşılaştırılabilir.

Yeniden derlemek için: etiketi çek, [`installer.yml`](../../../.github/workflows/installer.yml)
içindeki derleme adımlarını izle (düz komutlardır) ve iki derlemenin SBOM'larını ile `bot.exe`
davranışını (`bot --version`) karşılaştır. Farklı bir özet normaldir. İndirilen bir dosyayı projenin
iş akışına bağlayan şey yeniden derleme değil, *attestation* kaydıdır.

## Bir denetim başarısız olursa

Yükleyiciyi çalıştırma. Onu yalnızca `emreocell/Praxi-Bot` deposunun
[sürümler sayfasından](https://github.com/emreocell/Praxi-Bot/releases) yeniden indir ve yeniden
denetle. Yine başarısız olursa lütfen [bir issue aç](https://github.com/emreocell/Praxi-Bot/issues)
ve hangi sürümün hangi denetiminin başarısız olduğunu yaz. Şüphelendiğin bir saldırı için herkese
açık bir issue yerine [SECURITY.md](../../../SECURITY.md) dosyasını izle.
