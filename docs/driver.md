# I driver di justOS

In justOS un driver è un **pacchetto**: un file `.jdp` che contiene un
manifesto di testo e il modulo compilato. Il kernel legge i manifesti, li
confronta con l'hardware e carica da solo il driver giusto. Installarne uno
vuol dire solo mettere il pacchetto dove il kernel lo trova.

## Come vengono scelti e installati

All'avvio il kernel enumera i dispositivi: il bus PCI e i dispositivi di
piattaforma (il controller PS/2 con tastiera e mouse). Ogni dispositivo
diventa un `DeviceInfo` con id, bus, vendor/device, classe, IRQ, risorse
(BAR) e, per quelli di piattaforma, un nome come `ps2-keyboard`.

Altri dispositivi li aggiungono i driver stessi: il driver di un
controller USB aggiunge ogni interfaccia di ogni dispositivo collegato
(`device_add`), come figlio del controller, e la toglie quando il
dispositivo viene scollegato (`device_remove`). Per quei dispositivi la
scelta del driver funziona esattamente come per quelli PCI.

I pacchetti stanno in tre posti:

| Dove | Cosa |
|---|---|
| `boot:/drivers` | il **pacchetto di avvio** (boot pack): i driver che servono prima che il disco di sistema sia montato. Il loader lo carica in memoria insieme al kernel |
| `boot:/system` | gli altri driver che vengono con il sistema, nello stesso pacchetto: arrivano e se ne vanno con la sua versione (vedi gli [aggiornamenti A/B](architettura.md#aggiornamenti-del-sistema-ab)) |
| `sys:/system/drivers` | il **magazzino** (driver store): i driver aggiunti dopo (`drv add`, `sys:/inbox`), anche versioni più nuove di quelli del sistema |

Per ogni dispositivo senza driver, il gestore:

1. valuta le regole `match` di tutti i manifesti. Una regola più specifica
   vale di più: un id PCI o USB esatto o un nome di piattaforma vale 100,
   `pci-class` o `usb-class` classe:sottoclasse:interfaccia 60,
   classe:sottoclasse 40, la sola classe 20;
2. a parità di punteggio sceglie la versione più recente; a parità di
   versione preferisce il magazzino ai driver del sistema, e questi al
   pacchetto di avvio;
3. carica il modulo (una volta sola, anche se serve a più dispositivi) e
   chiama `probe` sul dispositivo. Se `probe` rifiuta, prova il candidato
   successivo.

Questo succede in tre momenti:

- **all'avvio**: prima con i driver di `boot:/drivers` (così si monta
  `sys:`), poi con tutti gli altri e il magazzino;
- **dopo ogni scansione** dei dispositivi (`dev scan`);
- **quando arriva un pacchetto nuovo**. Il thread `drvmon` controlla
  `sys:/inbox` ogni due secondi. Un pacchetto lasciato lì viene verificato
  (formato, CRC, manifesto, versione dell'ABI), spostato nel magazzino e
  installato. Un pacchetto non valido viene rinominato in
  `<nome>.jdp.rejected`, con il motivo nel giornale (`log`).

Se arriva una versione più nuova di un driver già in esecuzione, la vecchia
viene scaricata e sostituita subito, se il driver lo permette (cioè se ha
`remove`). Altrimenti la sostituzione avviene al riavvio.

### Comandi

```
dev                  dispositivi, con il driver di ognuno
dev 5                dettagli del dispositivo 5
dev scan             cerca hardware nuovo e installa i driver mancanti
drv                  tutti i driver: dove si trovano e se sono in uso
drv info <nome>      manifesto, file, dispositivi gestiti
drv add <file.jdp>   installa un pacchetto (come l'inbox, ma subito)
drv remove <nome>    toglie il driver dal magazzino e, se può, lo scarica
drv reload <nome>    scarica e ricarica un driver
drv inbox            controlla l'inbox adesso
```

Dal computer di sviluppo si può mettere un pacchetto nell'inbox di
un'immagine senza avviarla:

```sh
tools/target/release/justtool fs build/justos.img put mio.jdp /inbox/
```

## Il manifesto

Righe `chiave = valore`. `#` inizia un commento. Le chiavi sconosciute
vengono ignorate, così un manifesto più nuovo si carica anche su un kernel
più vecchio.

```text
# Intel PRO/1000 Ethernet.
name        = e1000                 # obbligatorio: nome unico del driver
version     = 1.0.0                 # obbligatorio: major.minor.patch
class       = network               # storage, input, network, display, other
description = Intel PRO/1000 Ethernet
author      = justOS
boot        = no                    # yes: va nel pacchetto di avvio
match       = pci 8086:100e         # vendor:device (esadecimale)
match       = pci 8086:10d3
```

Le regole `match` possibili:

| Regola | Esempio | Significato |
|---|---|---|
| `pci VVVV:DDDD` | `pci 1af4:1001` | un modello preciso |
| `pci-class CC[:SS[:PP]]` | `pci-class 01:06:01` | una classe PCI (qui: SATA AHCI) |
| `usb VVVV:PPPP` | `usb 0781:5567` | un modello USB preciso (vendor:product) |
| `usb-class CC[:SS[:PP]]` | `usb-class 08:06:50` | una classe di interfaccia USB (qui: disco, SCSI, bulk-only) |
| `platform <id>` | `platform ps2-mouse` | un dispositivo di piattaforma |

Un manifesto senza regole è valido, ma il driver non verrà mai scelto
automaticamente (`justtool pack` lo segnala).

## Scrivere un driver

Un driver è un crate Rust `no_std` nella cartella `system/drivers/<nome>/`,
che usa la libreria `justdrv`. Il `Makefile` trova da solo ogni cartella
lì dentro, la compila, ne fa un pacchetto e lo mette nel magazzino
dell'immagine (e nel pacchetto di avvio se il manifesto dice `boot = yes`).

```
system/drivers/esempio/
├── Cargo.toml
├── build.rs
├── driver.manifest
└── src/main.rs
```

`Cargo.toml`:

```toml
[package]
name = "esempio"
description = "justOS driver: esempio"
version = "1.0.0"
edition.workspace = true
publish.workspace = true
authors.workspace = true

[[bin]]
name = "esempio"
path = "src/main.rs"
test = false
bench = false

[dependencies]
justdrv.workspace = true

[lints]
workspace = true
```

`build.rs` (uguale per tutti i driver: un'immagine indipendente dalla
posizione, con il punto d'ingresso `just_driver_entry`):

```rust
fn main() {
    println!("cargo:rustc-link-arg-bins=-e");
    println!("cargo:rustc-link-arg-bins=just_driver_entry");
    println!("cargo:rustc-link-arg-bins=--strip-all");
    println!("cargo:rustc-link-arg-bins=-z");
    println!("cargo:rustc-link-arg-bins=max-page-size=4096");
}
```

`src/main.rs`:

```rust
#![no_std]
#![no_main]

use justdrv::{DeviceInfo, IrqLock, Mmio, Pci, Status, E_NODEV, OK};
use core::ffi::c_void;

justdrv::driver! {
    name: "esempio",
    version: "1.0.0",
    probe: probe,
    remove: remove,      // facoltativo: senza, il driver non si può scaricare
}

/// I registri del dispositivo, finché il driver lo gestisce.
static REGS: IrqLock<Option<Mmio>> = IrqLock::new(None);

/// Chiamata per ogni dispositivo che il manifesto ha scelto.
fn probe(dev: &DeviceInfo) -> Status {
    let bar = dev.resources[0];
    let Some(regs) = Mmio::map(bar.base, bar.size as usize) else {
        return E_NODEV; // il kernel prova il candidato successivo
    };
    Pci(dev.id).enable(false, true, true); // memoria e bus master
    let version = regs.read32(0);
    *REGS.lock() = Some(regs);
    let status = justdrv::irq_attach(dev.id, dev.irq, irq, core::ptr::null_mut());
    if status != OK {
        return status;
    }
    justdrv::info!("versione {version:#x}, IRQ {}", dev.irq);
    OK
}

unsafe extern "C" fn irq(_ctx: *mut c_void) {
    if let Some(regs) = REGS.lock().as_ref() {
        regs.write32(4, 1); // per esempio: conferma l'interruzione
    }
}

fn remove(device: u32) {
    justdrv::irq_detach(device, 0);
    REGS.lock().take();
}
```

I driver in `system/drivers/` sono esempi completi, dal più semplice,
`ps2kbd`, a `nvme`.

### Cosa offre il kernel

Un modulo non vede il kernel: riceve solo una tabella di funzioni
(`KernelApi`, in `system/libs/justdk/src/abi.rs`), che `justdrv` avvolge in
funzioni sicure:

| Servizio | In `justdrv` |
|---|---|
| giornale | `info!`, `warn!`, `error!`, `debug!` (il prefisso col nome del driver è automatico) |
| memoria | allocatore globale: `alloc::vec::Vec`, `Box`, `String` funzionano |
| registri | `Mmio` (memoria del dispositivo, mappata non in cache), `port::inb/outb/...`, `Regs` (la stessa interfaccia per porte e memoria, per i dispositivi che hanno entrambe) |
| DMA | `Dma`: memoria fisicamente contigua, sotto i 4 GiB, con il suo indirizzo fisico |
| PCI | `Pci`: spazio di configurazione, bus master, `find_capability` |
| interruzioni | `irq_attach`, `irq_detach` per la linea classica; `irq_attach_msi` per MSI-X o MSI; `irq_attach_any` prova MSI e ripiega sulla linea |
| attese | `Event`: un thread aspetta (con un limite di tempo) che un altro thread o un gestore di interruzione lo svegli |
| dispositivi figli | `device_add`, `device_remove` (per i driver di bus, come quelli USB) |
| tempo e thread | `time_ms`, `sleep_ms`, `yield_now`, `wait_until`, `spawn` |
| sincronizzazione | `IrqLock` (spinlock con interruzioni spente), `Mutex` |
| dischi | `block_register` con un `BlockDesc` (lettura, scrittura, flush) |
| input | `input_event` (tasti come usage HID, movimenti, pulsanti) |
| rete | `net_register` con un `NetDesc`, `net_receive` per i frame in arrivo |
| Bluetooth | `hci_register` con un `HciDesc` (la funzione che manda comandi e dati ACL al controller), `hci_receive` per gli eventi e i dati in arrivo (vedi [Bluetooth](#bluetooth)) |
| firmware | `firmware(nome)`: un file di `sys:/system/firmware/` (per i chip che lo vogliono caricato all'avvio) |
| Wi-Fi | `wifi_register` con un `WifiDesc` (indirizzo MAC, canali, invio di un frame 802.11, cambio di canale, `set_link` per la rete a cui la stazione si è associata), `wifi_receive` per i frame in arrivo con canale e potenza del segnale. La scheda manda e riceve frame 802.11 grezzi: ricerca delle reti, autenticazione, WPA2/WPA3 e cifratura li fa il kernel (`justwifi`), non il driver |
| suono | `audio_register` con un `AudioDesc` (scrittura di campioni a 16 bit, svuotamento); `audio_queued` dice al kernel quanti campioni la scheda ha ancora da suonare; `pcm::PcmRing` gestisce il buffer circolare che quasi ogni scheda audio legge in loop |
| registrazione | `capture_register` con un `CaptureDesc` (accensione e spegnimento dell'ingresso, lettura di campioni a 16 bit); `pcm::CaptureRing` gestisce il buffer circolare in cui la scheda scrive (vedi sotto) |
| schermo | `display_register` con un `DisplayDesc` (modi, cambio di modo, aggiornamento di un rettangolo); `map_framebuffer` mappa la memoria video in write-combining; `display_heads` e `display_changed` per più monitor (vedi sotto) |
| sospensione | `power_hooks`: come il dispositivo si ferma prima della sospensione e riparte al risveglio (vedi sotto) |

Il driver dichiara la versione dell'ABI per cui è compilato
(`ABI_VERSION`, oggi 2). Il kernel rifiuta i pacchetti di un'ABI diversa
invece di caricarli e rischiare un errore. L'ABI 2 ha aggiunto MSI, gli
eventi e i dispositivi figli; i servizi nuovi si aggiungono in fondo alla
tabella, quindi un driver compilato per l'ABI 2 funziona anche con un
kernel più recente della stessa ABI (non il contrario: i driver audio e
video della versione 0.4 chiedono un kernel 0.4). Gli ultimi servizi
aggiunti sono `audio_queued`, con cui il mixer del kernel sa quanto suono è
ancora nella scheda e dà ai programmi un orologio preciso (la latenza che
PulseAudio riporta; un driver che non lo chiama funziona lo stesso, e il
kernel stima la coda dal tempo), e `capture_register`, in fondo alla
tabella: i driver di prima, che non lo conoscono, funzionano come prima;
un driver che lo usa chiede un kernel che lo ha (`justdrv` controlla la
lunghezza della tabella).

### Registrazione

Un driver audio con un ingresso (microfono, linea) lo registra con
`capture_register` e un `CaptureDesc`: la frequenza e i canali in cui
registra, `capture(ctx, on)` per accendere e spegnere l'ingresso e
`read(ctx, campioni, frame)`, che copia i frame arrivati (interi a 16 bit,
interlacciati) e ne dice il numero, aspettando al più qualche decina di
millisecondi. Il kernel accende l'ingresso solo quando qualcuno registra
dal microfono e lo spegne quando nessuno lo fa più: un thread del kernel
(`capture`) legge 10 ms alla volta, converte in stereo, applica il volume
d'ingresso e passa il suono a ogni registrazione alla sua frequenza
(`dev/capture`). Senza ingresso le registrazioni ricevono silenzio al
ritmo dell'orologio, a 48 kHz, come la riproduzione senza scheda.

`pcm::CaptureRing` è il gemello di `PcmRing`: un buffer DMA in periodi che
la scheda riempie in loop; l'interruzione di fine periodo chiama
`progress.signal()` e `read` copia da dove il lettore era arrivato fino a
dove è la scheda. Chi resta indietro di più di tre quarti del buffer
salta a mezzo buffer dalla scheda (perde il suono vecchio invece di
leggere dati sovrascritti). Anche lui ha `pause` e `unpause` per la
sospensione.

Il driver `hda` (1.2.0) cerca nel codec un convertitore d'ingresso (ADC)
con un percorso, attraverso mixer e selettori, fino a un pin d'ingresso:
prima un microfono, poi un ingresso di linea, poi qualunque pin che sa
ricevere. Accende il pin (con la polarizzazione per i microfoni, quando
c'è), toglie il muto e mette il guadagno a 0 dB lungo il percorso, e usa
il primo stream d'ingresso del controller a 48 kHz, 16 bit, stereo. In
QEMU lo provano `hda-duplex` e `hda-micro`, che hanno un ingresso;
`hda-output` no, e la scheda resta solo d'uscita.

Il kernel converte ogni suono al formato dell'uscita (frequenza, canali) e
applica il volume da sé: un driver audio dichiara un solo formato e lascia
il guadagno dell'hardware fisso. Un driver video descrive il framebuffer
di ogni modo; se il dispositivo non legge da solo la memoria che justOS
disegna (virtio-gpu, VMware SVGA), fornisce anche `flush`, che il kernel
chiama per ogni rettangolo cambiato.

### Più monitor

Un driver video con più uscite si registra come sempre con
`display_register` (un monitor, quello dell'uscita 0: i driver che non
sanno altro restano così) e poi dichiara le uscite:

```rust
justdrv::display_heads(n as u32, ctx, outputs, set_heads);
```

- `outputs(ctx, out, max) -> usize` scrive un `DisplayOutput` per uscita
  (`width` e `height` preferiti dal monitor, `flags` con `OUTPUT_CONNECTED`
  se ce n'è uno collegato) e dice quante sono;
- `set_heads(ctx, size, heads, count, fb) -> Status` prepara un solo
  framebuffer di `size` e lo divide fra le uscite: ogni `DisplayHead` dice
  quale uscita (`output`) mostra quale rettangolo (`x`, `y`, `width`,
  `height`). Le uscite che non sono nella lista si spengono. Il kernel
  passa sempre monitor della stessa misura, in fila con `y = 0` o tutti a
  `0,0` (*Duplica*); `flush` riceve rettangoli dell'intero framebuffer;
- `display_changed(n)` avvisa che un monitor è stato collegato o staccato.
  Il kernel non richiama il driver da lì: è la scrivania, dal suo thread,
  a rileggere le uscite e a chiedere di nuovo `set_heads`.

`display_heads` e `display_changed` sono in fondo alla `KernelApi`
(l'ABI resta la 2): un driver che le usa chiede un kernel che le abbia, e
`justdrv::init` lo rifiuta su uno più vecchio, come per gli altri servizi
aggiunti. `virtiogpu` 1.2 le implementa: `GET_DISPLAY_INFO` per le uscite,
una risorsa larga quanto tutti i monitor e un `SET_SCANOUT` per ognuno, e
un thread che legge `events_read` due volte al secondo per
`VIRTIO_GPU_EVENT_DISPLAY`. La linea di interruzione del dispositivo è
spenta (bit *interrupt disable* del registro di comando PCI): l'evento
alza l'interruzione di configurazione, e una linea che nessuno conferma
tiene il processore nelle interruzioni.
`make test-display` ha il caso `virtio2`: due monitor, prima duplicati, poi
affiancati con `display mirror off`, ognuno con la sua metà dello schermo.

### Più dispositivi, interruzioni e polling

Un driver può gestire più dispositivi uguali (due schede di rete, due
controller USB): `probe` viene chiamata per ognuno e lo stato va tenuto per
dispositivo, non in variabili globali. Il modo più semplice è un `Box`
lasciato vivo con `Box::leak` e passato come `ctx` alle interruzioni e ai
thread.

Le linee di interruzione classiche sono spesso condivise: un gestore deve
controllare prima di tutto se l'interruzione è del suo dispositivo e
tornare subito se non lo è, ma deve sempre confermare quelle del suo
dispositivo, anche se non gli servono. Una linea lasciata alta blocca
anche tutti gli altri dispositivi collegati. I driver inclusi tengono
anche un thread di polling come rete di sicurezza, perché su alcune schede
madri le interruzioni arrivano instradate male.

### Sospensione

Durante la sospensione in RAM (S3) la memoria resta alimentata, i
dispositivi no: al risveglio ogni registro è tornato al valore di reset.
In s2idle la corrente resta, ma i dispositivi si fermano allo stesso
modo: gli stessi hook servono a entrambe.
Un driver dice come il suo dispositivo si ferma e riparte, in `probe`:

```rust
justdrv::power_hooks(dev.id, ctx, Some(suspend), resume);

unsafe extern "C" fn suspend(ctx: *mut c_void) -> Status { /* ... */ OK }
unsafe extern "C" fn resume(ctx: *mut c_void) -> Status { /* ... */ OK }
```

- `suspend` (facoltativa) ferma il dispositivo: finisce o abbandona quello
  che stava facendo e non fa più DMA. Un errore tiene sveglia la macchina
  (i dispositivi già fermati ripartono).
- `resume` lo rimette come lo aveva lasciato `probe`. Il kernel ha già
  ripristinato la configurazione PCI (BAR, registro di comando, MSI e
  MSI-X).

Il kernel ferma i dispositivi dai figli ai genitori e li fa ripartire nel
verso opposto. Da prima di `suspend` a dopo `resume` trattiene l'I/O
diretto al dispositivo: i trasferimenti dei dischi aspettano, i frame di
rete in uscita si scartano, il suono aspetta. Un dispositivo con un driver
ma senza hook impedisce la sospensione: `suspend check` dice quale. I
dispositivi aggiunti da un driver (quelli USB) sono affare del genitore.

Le regole, imparate scrivendo gli hook dei driver inclusi:

- **mai aspettare tenendo un `IrqLock`**: con le interruzioni spente il
  tempo non avanza, quindi `wait_until` e `sleep_ms` non tornano più. Il
  reset del chip (che aspetta) va fatto fuori dal lock, la
  riconfigurazione (che scrive solo registri) dentro;
- le schede di rete tengono un `paused: AtomicBool`: mentre è vero,
  l'interruzione e la ricezione tornano subito e `send` risponde `E_IO`.
  `suspend` lo alza, prende e lascia il lock (così nessuno è a metà di un
  invio), maschera le interruzioni e ferma le unità; `resume` resetta,
  riconfigura (anelli vuoti, MAC, filtri) e lo abbassa;
- l'audio usa `PcmRing::pause` e `PcmRing::unpause` (e
  `CaptureRing::pause` e `unpause` per l'ingresso): chi scrive campioni
  aspetta invece di trovare il buffer fermo, chi registra riceve meno
  suono;
- un lettore SD rilegge la scheda al risveglio e la confronta (CID e
  indirizzamento) con quella di prima: se è un'altra, o non c'è più, la
  toglie come se fosse stata estratta;
- uno schermo rimette il modo che aveva. Se era ancora quello scelto dal
  firmware all'avvio, il kernel chiede al driver lo stesso modo al
  risveglio (il BIOS di QEMU rimette la scheda in modo testo).

Con `SLEEP=1`, `make test-drivers` e `make test-audio` sospendono la
macchina prima di ogni prova; `make test-display` ha i casi `*-sleep`,
`make test-acpi` i casi `sleep-pc`, `sleep-q35` e `sleep-uefi`.

## Driver USB

I driver USB sono di due tipi.

**Driver di controller** (`xhci`, `ehci`, `uhci`, `ohci`): parlano con il
controller e implementano il trait `justusb::Host`. Il trait chiede le
operazioni proprie del controller: stato, reset e indirizzo delle porte,
trasferimenti di controllo, bulk e interrupt, e la configurazione degli
endpoint. Il resto lo fa la libreria `justusb`, uguale per tutti:

- controlla le porte ogni 250 ms e a ogni cambiamento segnalato dal
  controller;
- enumera i dispositivi nuovi: indirizzo, descrittori, nomi, configurazione;
- gestisce gli hub USB 2 e USB 3, anche uno dentro l'altro;
- aggiunge al kernel ogni interfaccia come dispositivo `usb`, e la toglie
  quando il dispositivo viene scollegato.

EHCI gestisce i dispositivi ad alta velocità (480 Mbit/s). Quelli più lenti collegati
direttamente a una sua porta passano ai controller compagni UHCI o OHCI;
quelli dietro un hub USB 2 usano le transazioni split dell'hub.

**Driver di classe** (`usbhid`, `usbstorage`): vengono scelti con le
regole `usb` e `usb-class`. In `DeviceInfo::interface` ricevono un
puntatore a `UsbInterface`, che contiene:

- il descrittore di configurazione completo;
- le funzioni per i trasferimenti: `control`, `transfer`, `poll_start`
  (un report periodico da un endpoint interrupt), `poll_stop` e
  `clear_halt`.

Non sanno su quale controller si trovano. Le funzioni per leggere i
descrittori sono in `justdrv::usb`.

Per la sospensione un driver di controller dichiara `const SLEEPS: bool =
true` e implementa `Host::stop` (il controller si ferma) e `Host::restart`
(reset e avvio come all'inizio, senza dispositivi). `justusb` fa il resto:
prima della sospensione toglie tutti i dispositivi collegati, al risveglio
li trova di nuovo e li enumera, e i driver di classe ripartono con `probe`.

### Bluetooth

`btusb` è il driver dei controller Bluetooth USB: la classe standard
(E0/01/01, cioè quasi ogni dongle e ogni scheda combinata Wi-Fi +
Bluetooth) più gli identificativi Realtek che non la dichiarano. Fa solo
il trasporto: i comandi HCI vanno con una richiesta di controllo
all'endpoint 0, gli eventi arrivano dall'endpoint interrupt, i dati ACL
dai due bulk. Il resto (HCI, L2CAP, SMP, GATT, i profili HID) è nel kernel
(`system/kernel/src/bt`), uguale per tutti i controller.

I chip Realtek partono dal codice della loro ROM e vogliono una patch a
ogni avvio, come fa `btrtl` su Linux: `btusb` legge la versione (Read
Local Version: sottoversione e revisione dicono il chip), sceglie i file
in `sys:/system/firmware/rtl_bt/`, chiede la versione della ROM
(comando 0xFC6D) e la chiave di sicurezza (0xFC61), prende dal file
`RTBTCore` le sottosezioni per quella ROM nell'ordine di priorità,
aggiunge la configurazione e manda tutto con 0xFC20 in pezzi da 252 byte;
poi un reset, e il controller ha la versione nuova. Per l'**RTL8852BU**
(13d3:3571, la metà Bluetooth dell'RTL8852BE) i file sono
`rtl8852bu_fw.bin` e `rtl8852bu_config.bin`, da linux-firmware, con la
loro licenza (`LICENCE.rtlwifi_firmware.txt`: si possono ridistribuire).

Lo stack del kernel tiene un controller alla volta e offre:

- la ricerca: BR/EDR (inquiry con i risultati estesi, i nomi) e LE (scansione
  attiva), insieme;
- l'associazione: Secure Simple Pairing per BR/EDR (Just Works, conferma
  del codice, codice da scrivere sulla tastiera) e SMP per LE (Secure
  Connections con P-256 e AES-CMAC, e il vecchio pairing "legacy"); le
  chiavi restano in `sys:/system/bluetooth.json`;
- HID classico (L2CAP 0x11 e 0x13, il descrittore letto con SDP) e HID
  su GATT (il servizio 0x1812, la mappa dei report, le notifiche): tasti
  e movimenti arrivano come quelli di una tastiera o di un mouse USB;
- il ricollegamento: le tastiere BR/EDR associate si ricollegano da sole,
  i dispositivi LE associati vengono cercati all'accensione.

Il servizio `just.bluetooth` lo apre alle app (con il permesso
*Bluetooth*), il comando `bt` alla shell, *Impostazioni → Bluetooth* e il
riquadro del centro di controllo all'utente.

Manca: l'audio (A2DP, SBC), più controller insieme, i dispositivi che
non sono tastiere o mouse.

`make test-bt` lo prova senza Bluetooth: `scripts/btemu.py` è un
RTL8852BU finto che QEMU collega al suo xHCI con `usb-redir` (il
protocollo usbredir su un socket; nessun dispositivo del computer passa
alla macchina virtuale). Risponde ai comandi Realtek e controlla che la
patch ricevuta sia quella che le regole di `btrtl` prendono dal file; ha
vicino una tastiera BR/EDR (che si associa scrivendo il codice) e un
mouse LE (Secure Connections, con la crittografia calcolata a parte in
Python). La prova associa entrambi, fa scrivere un comando alla tastiera e
muovere il mouse, e controlla il risultato. È un'emulazione scritta dalle
stesse specifiche: il chip vero si conferma solo sul computer vero.

## Driver virtio

`justvirtio` è la parte comune ai driver virtio (`virtioblk`, `virtionet`).
Trova l'interfaccia moderna (virtio 1.0, con le capability PCI) o quella
legacy (porte I/O), negozia le funzioni e prepara le code. Offre anche le
interruzioni MSI-X o la linea classica. Un driver virtio nuovo deve solo
descrivere le sue code e i suoi messaggi.

Al risveglio `Virtio::restore` resetta il dispositivo, rinegozia le stesse
funzioni, restituisce ogni coda vuota e rimette i vettori MSI-X; il driver
riempie di nuovo le code e chiama `ready`. Quando le code stanno dietro un
lock che non si può tenere durante il reset, gli stessi passi ci sono
separati: `negotiate`, `requeue` per ogni coda, `reroute`.

## Hardware supportato

"Provato" vuol dire verificato in QEMU da `make test-drivers` (dischi,
rete, USB, lettori SD), `make test-audio` (il suono registrato da QEMU
deve avere l'altezza e la durata giuste) o `make test-display` (lo schermo
che QEMU mostra deve avere la nuova risoluzione), e a mano per il
collegamento a caldo. Gli altri id elencati nei manifesti sono
chip della stessa famiglia che usano la stessa interfaccia.

"Provato su modello" vuol dire provato su un modello del chip scritto per
justOS, perché QEMU non lo emula: il sorgente è in `tools/qemu/` e
`scripts/build-qemu.sh` costruisce un QEMU che lo contiene. Il modello è
severo: segnala (con `-d guest_errors`, righe che iniziano con il nome del
chip) ciò che il chip vero rifiuterebbe o ignorerebbe, e `make
test-drivers` fallisce se ne compare una. Resta un modello: i dettagli
delle singole revisioni del chip non ci sono.

Il test di rete fallisce anche se un ping impiega 20 ms o più: le risposte
arrivano allora dal thread di polling del driver, cioè le interruzioni
della scheda non funzionano.

| Driver | Hardware | Stato |
|---|---|---|
| `ata` | controller IDE/PATA | provato |
| `ahci` | controller SATA AHCI (qualunque marca, per classe PCI) | provato |
| `nvme` | dischi NVMe (per classe PCI) | provato |
| `virtioblk` | dischi virtio (interfaccia legacy e moderna) | provato |
| `xhci` | controller USB 3 (per classe PCI) | provato, anche con hub e collegamento a caldo |
| `ehci` | controller USB 2 (per classe PCI), con i compagni UHCI/OHCI | provato, anche con collegamento a caldo |
| `uhci` | controller USB 1.1 Intel/VIA (per classe PCI) | provato, anche con hub |
| `ohci` | controller USB 1.1 degli altri produttori (per classe PCI) | provato, anche con hub |
| `usbhid` | tastiere, mouse e tavolette USB | provato |
| `usbstorage` | chiavette e dischi USB (bulk-only, SCSI) | provato, anche come disco di sistema |
| `btusb` | Bluetooth USB (classe E0/01/01), Realtek RTL8852BU/8852AU/8852CU/8761BU/8822CU con il firmware | provato su un RTL8852BU emulato (`make test-bt`); sul chip vero non verificato |
| `e1000` | Intel 82540/82545/82574 | provato |
| `e1000` | Intel 8254x, 8257x, 82583 | stessa interfaccia, non provato su hardware |
| `e1000` | Intel 82577/82578/82579, I217, I218, I219 | non verificato su hardware |
| `igb` | Intel 82576 | provato (MSI-X) |
| `igb` | Intel 82575, 82580, I350, I354, I210, I211, I225, I226 (2,5 Gbit) | stessa interfaccia, non provato su hardware |
| `rtl8139` | Realtek RTL8139 e schede derivate | provato |
| `pcnet` | AMD PCnet-PCI II e successive | provato |
| `virtionet` | schede di rete virtio (legacy e moderna) | provato |
| `rtl8169` | Realtek RTL8169 (I/O, linea classica), RTL8111/8168 (MSI, MSI-X), RTL8125 2,5 Gbit (MSI-X) | provato su modello |
| `rtl8169` | Realtek RTL810x e le altre revisioni | stessa interfaccia, non provato su hardware |
| `rtw8852be` | Wi-Fi Realtek RTL8852BE (`10ec:b852`, `10ec:b85b`) | **non finito**: accende il chip, legge revisione ed efuse (MAC), sceglie e controlla il firmware, poi si ferma; manca il caricamento del firmware e tutto il resto. Non provato su hardware |
| `ps2kbd`, `ps2mouse` | tastiera e mouse PS/2 | provati |
| `sdhci` | lettori di schede SD/MMC standard (per classe PCI): Intel, O2 Micro, Ricoh, AMD | provato con ADMA2, SDMA e senza DMA, schede SD e SDHC |
| `hda` | audio HD Intel (per classe PCI), codec Realtek, Conexant, IDT, VIA, Cirrus | provato con uscita di linea e altoparlante, e la registrazione da microfono e ingresso di linea (`hda-duplex`); sui codec veri non verificato |
| `ac97` | audio AC'97 Intel ICH, SiS, nVidia, AMD | provato (82801AA) |
| `bochsvga` | schermo di QEMU (VGA standard, bochs-display), Bochs, VirtualBox VBoxVGA, QXL | provato in QEMU |
| `vmsvga` | VMware SVGA II: VMware, VirtualBox VMSVGA | provato in QEMU |
| `virtiogpu` | virtio GPU (2D), fino a 16 monitor | provato |
| `cirrus` | Cirrus Logic GD5446 (QEMU `-vga cirrus`), 640x480 e 800x600 | provato in QEMU |
| `fbgeneric` | qualunque altra scheda (Intel, AMD, NVIDIA, Hyper-V, ATI di QEMU…): usa il framebuffer del firmware (GOP o VESA) | provato in QEMU (`ati-vga`) |

`fbgeneric` non cambia risoluzione: tiene il modo scelto dal firmware. Il
loader UEFI sceglie un modo grande del GOP (non più 1024x768 fisso), così
anche sulle schede vere lo schermo è nitido.

Il pulsante di accensione, la batteria, l'alimentatore, il coperchio e le
temperature dei portatili non hanno un driver: li gestisce il kernel
eseguendo l'AML del firmware (ACPI, vedi
[architettura](architettura.md#acpi-e-aml), `make test-acpi`).

Il Wi-Fi c'è sopra il driver: la stazione del kernel (`justwifi`: reti
aperte, WPA2-PSK, WPA3-SAE con PMF, CCMP) è provata con `make test-wifi`,
dove una scheda virtio porta i frame 802.11 fino a un simulatore di access
point (`scripts/wifi/apsim.py`). Manca un driver finito per una scheda
vera: `rtw8852be` è il primo, a metà.

Mancano ancora i driver per: Wi-Fi (finiti), l'audio Bluetooth, l'accelerazione e il cambio di
risoluzione delle schede video vere (Intel, AMD, NVIDIA: hanno lo schermo
tramite `fbgeneric`), lettori di schede Realtek e Genesys su PCIe.

### Il pacchetto

Con `make` i pacchetti si creano da soli in `build/drivers/`. A mano:

```sh
cd system && cargo build --release -p esempio
tools/target/release/justtool pack system/drivers/esempio/driver.manifest \
    system/target/x86_64-unknown-none/release/esempio esempio.jdp
tools/target/release/justtool inspect esempio.jdp
```

`justtool pack` controlla il manifesto e che il modulo sia un'immagine
caricabile: ELF64, indipendente dalla posizione, con le sole rilocazioni
relative e il punto d'ingresso giusto.

Formato `.jdp` (interi little-endian):

```text
offset  dim.  campo
     0     8  "JUSTDRVP"
     8     4  versione del formato (1)
    12     4  versione dell'ABI dei driver
    16     4  posizione del manifesto    20  4  lunghezza
    24     4  posizione del modulo       28  4  lunghezza
    32     4  CRC-32 di tutto ciò che segue l'intestazione
    36     4  riservato
    40        manifesto (UTF-8), modulo (ELF64)
```
