# tincan

Terminalde çalışan, sunucusuz sesli sohbet. Discord'un yaptığı işi, kimsenin sunucusuna
ihtiyaç duymadan yapar: uygulamayı ilk açan kişi odayı kurar, ürettiği davet kodunu
arkadaşlarına gönderir, onlar da dünyanın herhangi bir yerinden o kodla bağlanır.
VPN yok, port yönlendirme yok, hesap açma yok.

```
┌ kanallar ──────────────┐┌ #genel · istanbul ─────────────────────────────┐
│>  🔊 genel   2         ││23:56 — mehmet odaya katıldı                    │
│      oyun              ││23:56 mehmet: merhaba ben mehmet                │
│      müzik             ││                                                │
└────────────────────────┘│                                                │
┌ kişiler (2) ───────────┐│                                                │
│● ahmet · genel         ││                                                │
│● mehmet (sen) · genel  │└────────────────────────────────────────────────┘
└────────────────────────┘┌────────────────────────────────────────────────┐
                          │#genel ▏                                        │
Tab kanal · F2 ses · F3 sustur · Ctrl+C çık          🔊 genel  kod: n73w-kuqc…
```

## Nasıl çalışır

İki düzlem birbirinden ayrı:

**Kontrol düzlemi (yıldız).** Odayı ilk açan kişi *koordinatördür*: üye listesi, kanallar ve
yazışma onun üzerinden akar. Trafiği küçüktür, saniyede birkaç yüz bayt.

**Ses düzlemi (mesh).** Aynı kanaldaki peer'lar birbirine doğrudan bağlanır ve Opus
paketlerini QUIC datagram olarak yollar. **Ses koordinatörden geçmez** — host'un bağlantısı
darboğaz olmaz, 6 kişilik bir odada kişi başı ~160 kbps upload yeter.

```
        [ahmet: koordinatör]
         /      |      \          ── kontrol (güvenilir stream)
     ayşe    mehmet   zeynep
        \______|______/           ── ses (mesh, doğrudan datagram)
```

Bağlantı için [iroh](https://iroh.computer) kullanılıyor: davet kodu peer'ın public key'idir.
Çoğu durumda doğrudan P2P bağlantı kurulur; NAT delme başarısız olursa trafik relay üzerinden
akar — relay içeriği çözemez, sadece iletir. QUIC her bağlantıyı uçtan uca şifreler ve karşı
tarafın kimliğini public key ile doğrular.

## Kurulum

Gereksinimler:

- **Rust 1.91+** (`rustup update stable`)
- **cmake** ve **pkg-config** — Opus kodeki kaynaktan derleniyor
  (macOS: `brew install cmake pkg-config`, Debian/Ubuntu: `apt install cmake pkg-config`)
- 48000 Hz çalışan bir mikrofon ve hoparlör

```bash
git clone <repo> && cd tincan-cli
cargo build --release
```

İlk çalıştırmada işletim sistemi mikrofon izni ister. macOS'ta izni isteyen, tincan değil
onu çalıştıran terminal uygulamasıdır (Terminal, iTerm, VS Code...).

## Kullanım

**Oda açmak:**

```bash
tincan host --name ahmet --room istanbul --password gizli
```

Ekrana bir davet kodu basar. Kodu arkadaşlarınıza gönderin (kopyala-yapıştır; 63 karakter).

**Odaya katılmak:**

```bash
tincan join n73w-kuqc-uog2-... --name mehmet --password gizli
```

**Ses cihazlarını görmek:**

```bash
tincan devices
```

### Seçenekler

| Seçenek | Açıklama |
|---|---|
| `--name`, `-n` | Odada görünecek takma adınız (varsayılan: sistem kullanıcı adı) |
| `--password`, `-p` | Oda parolası. Verilmezse kodu bilen herkes girebilir |
| `--room` | Oda adı (yalnızca `host`) |
| `--channels` | Virgülle ayrılmış kanal listesi (varsayılan: `genel,oyun,müzik`) |
| `--no-voice` | Sesi hiç açma; yalnızca yazışma |

### Kısayollar

| Tuş | İş |
|---|---|
| `Tab` / `Shift+Tab` | Kanallar arasında gezin |
| `F2` (veya `Ctrl+G`) | Bakılan kanalın sesine gir / çık |
| `F3` (veya `Ctrl+T`) | Mikrofonu sustur / aç |
| `Enter` | Mesajı gönder |
| `Ctrl+C` | Çık |

Bakılan kanal ile sesle bağlı olunan kanal birbirinden bağımsızdır: "oyun"da konuşurken
"genel"deki yazışmayı okuyabilirsiniz. Kanal listesinde `>` baktığınız kanalı, `🔊` sesle
bağlı olduğunuz kanalı gösterir.

Ses kısayolları bilerek F-tuşları: terminalde `Ctrl+M` (0x0D) ve `Ctrl+J` (0x0A) Enter'ın
kendisidir, ondan ayırt edilemez — onlar kullanılsaydı "sustur" tuşu sessizce mesaj gönderirdi.

## Güvenlik

Parola tel üzerinden hiç geçmez: koordinatör rastgele bir nonce yollar, istemci
`Argon2id(parola, nonce)` sonucunu geri gönderir. Nonce her bağlantıda yenilendiği için
yakalanan bir kanıt tekrar kullanılamaz.

Parola şifreleme için değil, **katılım denetimi** içindir — şifrelemeyi QUIC zaten yapıyor.

> `--password` komut satırında görünür, yani aynı makinedeki başka kullanıcılar `ps` ile
> okuyabilir. Paylaşımlı bir makinedeyseniz bunu aklınızda tutun.

## Geliştirme

```bash
cargo test              # 82 test: birim + kontrol düzlemi + ses mesh'i
cargo clippy --all-targets
RUST_LOG=tincan=debug cargo run -- host 2>tincan.log   # loglar arayüzü bozmasın diye dosyaya
```

Testler internete çıkmaz: kontrol düzlemi ve ses mesh'i testleri gerçek iki iroh endpoint'i
ve gerçek QUIC bağlantısı kullanır ama relay ve keşif kapalıdır, adresler elle tanıtılır.
Ses testleri ses donanımı da kullanmaz — mesh'in uçlarına doğrudan bağlanırlar.

### Kaynak düzeni

```
src/
  proto.rs        Tel üzerindeki tipler (kontrol mesajları + ses paketi başlığı)
  room.rs         Odanın otoriter durumu — koordinatörün tek gerçek kaynağı, saf ve testli
  auth.rs         Parola kanıtı (Argon2id + nonce)
  invite.rs       Davet kodu: base32, gruplu, yapıştırmaya toleranslı
  net/
    endpoint.rs   iroh endpoint kurulumu, kimlik dönüşümleri
    control.rs    Koordinatör sunucusu + katılan istemci
    voice.rs      Ses mesh'i: bağlantı yönetimi, datagram taşıma, kanal filtresi
  audio/
    device.rs     cpal ↔ kilitsiz ring buffer köprüsü
    codec.rs      Opus kodlama/çözme + kayıp örtme
    jitter.rs     Peer başına jitter tamponu
    mixer.rs      Çok kaynaklı miksaj + limitleyici
    vad.rs        Konuşma algılama (gösterge + DTX)
  ui/
    state.rs      Arayüz durumu (ağdan ve terminalden bağımsız, testli)
    view.rs       Ekran düzeni
examples/         Faz 0 probe'ları — atılabilir ölçüm araçları
```

`examples/ping.rs` iki makine arasındaki bağlantıyı ve gecikmeyi ölçer,
`examples/loopback.rs` ses zincirini ölçer. İkisi de tasarım kararlarını doğrulamak için
yazıldı, üründe kullanılmıyor.

## Bilinen sınırlar

- **Koordinatör tek hata noktası.** Host çıkarsa oda dağılır. Lider devri bilinçli olarak
  MVP dışında bırakıldı.
- **48 kHz zorunlu.** Yeniden örnekleme yok; cihazınız farklı bir hızda çalışıyorsa tincan
  bunu açıkça söyleyip yazışma moduna düşer, sessizce bozuk ses üretmez.
- **Davet kodu 63 karakter.** Public key'in kendisi olduğu için kısaltılamaz —
  kopyala-yapıştır için sorun değil, telefonda okumak için uygun değil.
- **Ölçek 2–6 kişi.** Mesh'te herkes herkese gönderir; 8+ kişide koordinatörün sesi
  mikslemesi (SFU) gerekir.
- **Push-to-talk yok.** Şimdilik açık mikrofon + konuşma algılama; sustur tuşu var.
- **Bağlantının ilk saniyesi relay üzerinden akar**, sonra doğrudan bağlantıya geçer.
  Odaya girdiğinizde ilk anlarda gecikme fark edebilirsiniz.
