justOS: un sistema operativo per PC x86-64 scritto da zero in Rust, su cui girano Firefox, i programmi Linux e quelli Windows.

![La scrivania di justOS](https://raw.githubusercontent.com/justapps-it/justOS/main/docs/screenshots/scrivania.png)

## Novità della 0.9.1

- **I programmi Linux hanno l'audio**: Firefox (YouTube, i siti, i suoni delle pagine) suona attraverso un piccolo server PulseAudio dentro il kernel, mescolato ai suoni di sistema e al volume delle Impostazioni. Senza scheda audio i programmi non si bloccano.
- **Firefox si presenta ai siti come Firefox per Windows**: le pagine di download propongono la versione `.exe`, che si avvia con un doppio clic grazie a Wine.
- I pacchetti `.deb` di Debian si installano con un doppio clic (o `deb install` nel Terminale), se le loro dipendenze sono già nel sistema.

## Novità della 0.9.0

- **La barra si trasforma**: invece di aggiungere una seconda barra, il dock stesso diventa un'isola al centro in basso per quello che succede (una notifica, il volume o la luminosità, un programma che si avvia, una canzone o un timer appena partiti, un timer che suona). Appena ci passi sopra con il puntatore torna la barra delle app; quando te ne vai l'isola ricompare.
- **Il dock non scatta più**: passandoci sopra, lo sfondo resta fermo e le icone crescono sul posto.
- **Più monitor**: con una scheda video che ne gestisce più di uno (per ora virtio-gpu nelle macchine virtuali), la scrivania si allarga su tutti; orologio e dock stanno sul monitor principale, le finestre passano da uno all'altro e si ingrandiscono sul loro. In Impostazioni → Schermo si sceglie il principale, l'ordine o la duplicazione, e la scelta resta.
- **Firefox apre le schede anche con 1 GiB di memoria**: un processo figlio condivide le pagine del padre finché uno dei due non scrive (`fork` copy-on-write), invece di copiarle tutte subito.
- **Firefox si chiude davvero**: prima, alla chiusura, restava bloccato un minuto e poi si fermava con un errore; ora esce da solo.
- **Sospensione anche sui portatili senza S3** (Modern Standby): s2idle, con i dispositivi fermi, lo schermo nero e i processori nel loro stato di riposo più profondo; si risveglia con il tasto di accensione, il coperchio, la tastiera o la sveglia.
- **Anteprime in File**: le immagini PNG, JPEG, GIF e QOI si vedono come miniature; il Visualizzatore apre anche PNG, JPEG e GIF.
- **La ricerca guarda anche dentro i file**: con Ctrl+Spazio, dopo i risultati per nome, la sezione *Contenuto* mostra i file di testo che contengono quello che scrivi, con la riga trovata.
- **Selettore di file dei programmi GTK** (Firefox compreso): si apre nella cartella home invece che su "Recenti" vuoto, i file recenti funzionano e non c'è più il Cestino che dava errore; le impostazioni dei programmi GTK restano da un avvio all'altro.
- **`help` del Terminale in italiano.**
- **Firefox, GTK e Wine in italiano**: menu, finestre di dialogo e messaggi dei programmi Linux e Windows sono tradotti; date e numeri seguono le regole italiane, e Firefox controlla l'ortografia in italiano.
- **Gli aggiornamenti arrivano da justapps-it**: l'indice firmato degli aggiornamenti si è spostato nella nuova casa del progetto. Chi ha installato la 0.8 vede l'annuncio della 0.9 in Info sistema, ma deve scaricarla a mano; dalla 0.9 in poi gli aggiornamenti si installano da soli.

![Programma che si avvia nella barra](https://raw.githubusercontent.com/justapps-it/justOS/main/docs/screenshots/isola.png)

## Cosa non va ancora

- Non si registra ancora dal microfono; l'audio dei programmi Windows con Wine non è ancora provato.
- Molti programmi Windows a 32 bit e quelli grandi (Steam, per esempio) si fermano: la compatibilità è agli inizi.
- Mancano Wi-Fi, Bluetooth e l'accelerazione grafica: sulle schede video vere (Intel, AMD, nVidia) si usa il framebuffer del firmware, alla risoluzione scelta dal firmware, con un solo monitor.
- La sospensione s2idle è provata in QEMU, non ancora su portatili veri; la retroilluminazione resta accesa (lo schermo diventa nero).
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
