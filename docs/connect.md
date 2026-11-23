# JustOS Connect

JustOS Connect collega un telefono Android al computer con justOS, sulla
stessa rete. Dal telefono si mandano file al computer e se ne ricevono, si
guarda lo schermo del computer e lo si tocca, si usa il telefono come
trackpad e tastiera. Le notifiche del telefono compaiono sulla scrivania,
la batteria si vede nella finestra di Connect, gli appunti passano dall'uno
all'altro, e dal computer si fa squillare il telefono per ritrovarlo.

Due parti:

* sul computer il servizio `connectd` del kernel
  ([`system/kernel/src/connect`](../system/kernel/src/connect)), la
  finestra **JustOS Connect** e il comando `connect` della shell;
* sul telefono l'app Android in [`apps/connect-android`](../apps/connect-android),
  scaricabile da [justos.dev/connect](https://justos.dev/connect).

## Abbinare un telefono

1. Telefono e computer sulla stessa rete (Wi-Fi o cavo, purché si vedano).
2. Sul computer, Connect è acceso: la finestra JustOS Connect (Ctrl+Spazio,
   «Connect») lo dice, e l'interruttore in alto lo accende e lo spegne.
3. Sul telefono, l'app JustOS Connect mostra i computer trovati: si tocca
   il proprio.
4. Il computer mostra un codice di sei cifre (nella finestra, e con
   `connect code` nella shell); lo si scrive sul telefono.

Da quel momento il telefono è *abbinato*: si ricollega da solo, senza
codice, e compare sotto «Abbinati». «Dimentica» (o `connect forget <nome>`)
lo toglie; per riaverlo serve di nuovo il codice.

## Cosa si fa

| Funzione | Come |
|---|---|
| File verso il computer | Dal telefono, «Invia file» o *Condividi → JustOS Connect* da qualsiasi app. Finiscono in `sys:/home/Scaricati`; se un nome c'è già diventa `nome (2).ext`. |
| File verso il telefono | Nella finestra, «Invia file» sotto il telefono, oppure `connect send <file>`. Finiscono nei Download del telefono. |
| Schermo | Dal telefono, «Schermo»: la scrivania in JPEG, una nuova immagine solo dopo che il telefono ha mostrato la precedente (quindi su una rete lenta scende il ritmo, non si accumula ritardo). Un tocco sull'immagine porta lì il puntatore. |
| Trackpad e tastiera | Un dito muove il puntatore, un tocco è un clic, due dita scorrono; la tastiera del telefono scrive sul computer, con Invio, frecce, Esc, F1–F12, volume e la vista d'insieme. |
| Notifiche | Con il permesso di accesso alle notifiche, quelle del telefono compaiono sulla scrivania. |
| Batteria | Il livello e se è in carica, nella finestra e in `connect`. |
| Appunti | Quello che si copia su uno si incolla sull'altro (testo). |
| Squillo | «Squilla» nella finestra, o `connect ring`: il telefono suona anche in silenzioso. |

## La shell

```
connect                      lo stato: acceso o spento, indirizzo, telefoni connessi e abbinati
connect on | off             accende o spegne il servizio
connect code                 il codice per il telefono che si sta abbinando
connect send <file>          manda un file al telefono connesso
connect ring                 fa squillare il telefono
connect forget <telefono>    dimentica un telefono abbinato (per nome o id)
```

## Sicurezza

Il computer risponde sulla rete locale soltanto: la ricerca è un pacchetto
UDP in broadcast, la connessione TCP. Ogni sessione è cifrata con una
chiave nuova (ECDH su P-256, poi AES-256-GCM in entrambe le direzioni);
un telefono abbinato dimostra di avere il suo token e il computer
dimostra di conoscerlo, senza mandarlo.

Il punto debole è l'abbinamento: sei cifre bastano contro chi indovina a
caso (il computer accetta un solo tentativo per codice), non contro chi
sta in mezzo sulla rete proprio mentre si abbina. **Abbinare su una rete
di cui ci si fida**, quella di casa, non il Wi-Fi di un bar.

Spento (`connect off`), il computer non risponde e chiude le sessioni.
Dimenticato, un telefono non entra più nemmeno se ha conservato il token.

## Il protocollo

Porte: **UDP 47800** per trovare i computer, **TCP 47801** per le sessioni.

Ricerca: il telefono manda `JUSTOS-CONNECT?` in broadcast; ogni computer
acceso risponde `JUSTOS-CONNECT {"id","name","port":47801,"ver":1}`.

Sul TCP ogni messaggio è una lunghezza (u32 big-endian) seguita dal corpo:
un byte di tipo e il contenuto. Dopo l'handshake il corpo è cifrato
(AES-256-GCM senza dati aggiuntivi; nonce = 4 byte a zero e un contatore
u64 big-endian per direzione).

Handshake:

1. `HELLO 0x01` (telefono, in chiaro) `{id, name, key}`: `key` è il punto
   P-256 non compresso (65 byte) in esadecimale.
2. `CHALLENGE 0x02` (computer, in chiaro) `{id, name, key, known, ver}`:
   `known` dice se il telefono è già abbinato.
3. Entrambi calcolano `th = SHA256(chiave_telefono || chiave_computer)`,
   `prk = HMAC-SHA256("justos-connect 1", segreto_ECDH)` e
   `okm = HKDF-Expand(prk, th, 64)`: i primi 32 byte cifrano dal telefono al
   computer, gli altri 32 dal computer al telefono.
4. Telefono abbinato: `AUTH 0x03 {mac: HMAC(token, "auth"||th)}`.
   Telefono nuovo: `PAIR 0x04`; il computer mostra il codice e il telefono
   manda `CODE 0x05 {mac: HMAC(codice, "pair"||th)}`.
5. `WELCOME 0x06 {id, name, w, h, gui, mac: HMAC(token, "pc"||th), token}`
   (`token` solo appena abbinato), oppure `DENIED 0x07 {reason}`.

Messaggi:

| Tipo | Codice | Contenuto |
|---|---|---|
| MOVE | `0x10` | i16 dx, i16 dy |
| BUTTON | `0x11` | u8 tasto (0 sinistro, 1 destro, 2 centrale), u8 premuto |
| SCROLL | `0x12` | i16 scatti |
| TEXT | `0x13` | testo UTF-8 |
| KEY | `0x14` | u8 codice, u8 modificatori (1 Ctrl, 2 Alt, 4 Maiusc), poi un carattere UTF-8 se il codice è 0 |
| TAP | `0x15` | u16 x, u16 y: frazioni (0–65535) dello schermo |
| FILE_BEGIN | `0x20` | `{name, size}` |
| FILE_DATA | `0x21` | al massimo 64 KiB |
| FILE_END · FILE_DONE · FILE_CANCEL | `0x22` · `0x23` · `0x24` | — · `{name, ok}` · — |
| SCREEN | `0x30` | `{on, w, q}`: acceso, larghezza massima, qualità JPEG |
| FRAME | `0x31` | u16 w, h, larghezza e altezza dello schermo, poi il JPEG |
| FRAME_ACK | `0x32` | l'immagine è stata mostrata |
| NOTIFY | `0x40` | `{app, title, text}` |
| BATTERY | `0x41` | `{level, charging}` |
| CLIP | `0x42` | testo degli appunti (in entrambe le direzioni) |
| PING · PONG | `0x50` · `0x51` | il computer chiude una sessione muta da 45 s |
| RING | `0x52` | fa squillare il telefono |

Codici di KEY: 1 Invio, 2 Backspace, 3 Canc, 4 Tab, 5 Esc, 6–9 frecce
(su, giù, sinistra, destra), poi Home, Fine, Pag su, Pag giù, volume su e
giù, muto (16), vista d'insieme (17), `0x21`–`0x2C` F1–F12.

Il computer tiene la sua configurazione in `sys:/system/connect`: una riga
`id <hex>`, `off` se spento, e una riga `device <id> <token> <nome>` per
ogni telefono abbinato.

## Provarlo

`scripts/test-connect.py <immagine>` fa la parte del telefono contro justOS
in QEMU (con una P-256 e un AES-GCM in Python puro): ricerca, abbinamento
con il codice letto dal log, un file in entrambe le direzioni con il
controllo SHA-256, due immagini dello schermo dopo aver mosso il puntatore,
batteria, notifica e appunti, ping, riconnessione con il token, un token
sbagliato rifiutato e un telefono dimenticato.

```
make test-image IMAGE=build/justos-t.img
scripts/test-connect.py build/justos-t.img
```
