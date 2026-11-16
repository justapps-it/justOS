justOS: un sistema operativo per PC x86-64 scritto da zero in Rust, su cui girano Firefox, i programmi Linux e quelli Windows.

![La scrivania di justOS](https://raw.githubusercontent.com/justapps-it/justOS/main/docs/screenshots/scrivania.png)

La prima versione pubblicata in [justapps-it/justOS](https://github.com/justapps-it/justOS), la nuova casa del progetto.

## Novità della 0.8

- **Programmi Linux veri**: gli eseguibili dinamici girano con la glibc di Debian, e un compositor Wayland dentro il kernel dà loro finestre di justOS. Funzionano GTK 3 e i programmi con un file `.desktop`, che compaiono in Applicazioni.
- **Firefox ESR è il browser di serie**, già installato nell'immagine di disco. Il browser scritto apposta, JustScript e JustStudio non ci sono più.
- **Programmi Windows con Wine 10**: un doppio clic su un `.exe`, `.msi` o `.bat` lo avvia, a 64 bit e, in parte, a 32 bit. La prima volta una finestra di justOS, *Programmi Windows*, prepara l'ambiente con una barra di avanzamento; i programmi installati compaiono in Applicazioni e nella ricerca.
- **Installazione sul disco del computer**: avviato da chiavetta o da CD, justOS propone di installarsi su un disco interno, con scelta del disco, conferma esplicita e barra di avanzamento. Il disco scelto viene cancellato per intero.
- **Primo avvio in 10 passi**: aspetto, tastiera, rete, account con password, nome del dispositivo, data e ora, privacy e aggiornamenti, sfondo.
- **Nuovo aspetto a pixel morbidi**, con tema scuro e tema chiaro, e un centro di controllo ridisegnato. A schermo intero il dock sale dal bordo in basso.
- **Ctrl+Maiusc+L** blocca lo schermo (prima era logo+L, che in una macchina virtuale su Windows restava "incastrato").

![Firefox ESR](https://raw.githubusercontent.com/justapps-it/justOS/main/docs/screenshots/firefox.png)

## Cosa non va ancora

- Molti programmi Windows a 32 bit e quelli grandi (Steam, per esempio) si fermano: la compatibilità è agli inizi.
- Mancano Wi-Fi, Bluetooth e l'accelerazione grafica: sulle schede video vere (Intel, AMD, nVidia) si usa il framebuffer del firmware, alla risoluzione scelta dal firmware.
- L'installazione su disco è provata in QEMU, non ancora su hardware vero; niente Secure Boot, dischi con settori da 4K o più grandi di 2 TiB.
- Lo stato di ogni driver è in [docs/driver.md](https://github.com/justapps-it/justOS/blob/main/docs/driver.md#hardware-supportato), cosa manca e in che ordine nella [roadmap](https://github.com/justapps-it/justOS/blob/main/docs/roadmap.md).

## File

| File | Cosa farne |
|---|---|
| `justos-img.zip` | **Immagine di disco** (scompattata 5 GiB, BIOS e UEFI), con Firefox e Wine: si scrive su una chiavetta USB da almeno 8 GB (balenaEtcher, Rufus in modalità DD, `dd`) per avviare un PC vero, o si usa come disco di una macchina virtuale. Secure Boot va disattivato. |
| `justos.iso` | **CD live**: si avvia in una macchina virtuale o da un CD, con BIOS o UEFI, senza disco rigido. Il sistema gira in memoria: le modifiche si perdono allo spegnimento. Non ha Firefox né Wine. |
| `SHA256SUMS` | le impronte dei due file |

## Provarlo

- **QEMU**, immagine di disco: `qemu-system-x86_64 -m 2G -smp 4 -drive file=justos.img,format=raw -device ich9-intel-hda -device hda-output` (con `-accel kvm` su Linux o `-accel whpx` su Windows va molto più veloce).
- **QEMU**, CD live: `qemu-system-x86_64 -m 1G -cdrom justos.iso`
- **VirtualBox**: nuova macchina "Other/Unknown (64-bit)", nessun disco, `justos.iso` nel lettore CD.

Al primo avvio justOS fa qualche domanda (tema, tastiera, account), poi apre la scrivania. Documentazione nel [README](https://github.com/justapps-it/justOS#readme).
