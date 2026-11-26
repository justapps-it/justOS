# L'interfaccia grafica

Quando il loader trova uno schermo grafico di almeno 800x600, justOS si avvia
in modalità grafica. Sullo schermo non compare mai testo tecnico: il kernel
scrive i suoi messaggi solo sulla porta seriale e nel giornale di sistema, e
le righe di comando si vedono soltanto aprendo il Terminale. Anche un errore
fatale mostra una schermata semplice («justOS si è dovuto fermare»), con i
dettagli solo sulla porta seriale.

## Avvio

1. **Splash**: logo, nome e una barra che avanza mentre il kernel avvia le sue
   parti (memoria, dischi, driver, rete) e carica icone e sfondi.
2. **Primo avvio**: una configurazione guidata in dieci passi, con una barra
   di puntini che mostra a che punto sei e uno scorrimento di 200 ms tra un
   passo e l'altro:
   1. *Benvenuto* (pulsante **INIZIA**; la lingua per ora è solo l'italiano);
   2. *Aspetto*: Scuro o Chiaro, con l'anteprima di una finestra; la scelta si
      applica subito a tutta la configurazione;
   3. *Tastiera*: Italiano o English (US), con un campo per provarla;
   4. *Rete*: per ogni scheda trovata mostra "Connesso" con l'indirizzo DHCP,
      "Cerco la rete…" oppure "Nessuna risposta" con il pulsante **Riprova**
      (rifà il DHCP); una riga dice se *Internet funziona* o c'è la *Rete
      locale soltanto* (risolve `raw.githubusercontent.com`, da cui arriva
      l'indice degli aggiornamenti, e si connette alla porta 443). Senza
      schede: "Nessuna scheda di rete". Il Wi-Fi non c'è ancora e la pagina lo
      dice solo se non c'è un cavo collegato. Si può sempre continuare senza
      rete. L'indirizzo fisso non è supportato dallo stack di rete e non è
      offerto;
   5. *Il tuo account*: nome e password facoltativa con conferma (la stessa
      password del blocco schermo, salvata con salt e hash);
   6. *Nome del dispositivo*: proposto dal nome ("Anna-PC"), validato come
      nome host (lettere senza accenti, cifre, trattini, 1-63 caratteri); è il
      nome che vede la rete (opzione 12 del DHCP), i programmi Linux
      (`uname -n`, `/etc/hostname`, `/proc/sys/kernel/hostname`), *Informazioni
      su justOS* e la shell (`hostname`, `sysinfo`);
   7. *Data e ora*: l'ora del computer con frecce per correggerla (scrive
      l'orologio CMOS). justOS non ha ancora i fusi orari: l'orologio è l'ora
      locale;
   8. *Privacy e aggiornamenti*: aggiornamenti automatici delle app, suono
      all'avvio, widget sulla scrivania;
   9. *Sfondo*;
   10. *Tutto pronto*: riepilogo e **Inizia**.

   Si usa con il mouse o con la tastiera (Invio per andare avanti, Esc per
   tornare indietro, Tab o frecce tra i campi). Con le impostazioni di
   partenza basta Invio a ogni passo, scrivendo il nome al passo *Il tuo
   account*. Niente viene salvato prima dell'ultimo passo: se si spegne prima,
   la configurazione riparte da capo. Alla fine crea le cartelle della home
   (Documenti, Scaricati, Immagini, Musica, Video, Scrivania, Progetti).
   Per l'automazione, nella shell `setup skip` la segna come fatta (`setup`
   mostra lo stato).
3. **Avvii successivi**: dopo lo splash compare direttamente la scrivania.

Le scelte sono in `sys:/system/settings` (il nome del dispositivo come
`device-name=`, modificabile in *Impostazioni → Generale → Nome del
dispositivo*); *Impostazioni → Generale → Configurazione iniziale* la ripropone al prossimo avvio. Il CD live non
conserva nulla, quindi mostra la configurazione ogni volta.

## La scrivania

Presa a modello da macOS:

- **Niente barra in alto**: l'ora e la data fluttuano al centro, con le
  cifre in pixel morbidi (bianche o nere secondo lo sfondo, con una
  piccola ombra). Un clic apre il **centro notifiche** con il calendario
  del mese e le notifiche ricevute.
- **La barra dinamica**: un'unica superficie di vetro scuro in basso al
  centro dello schermo principale, che prende il posto del dock, dell'isola,
  del centro di controllo, di Applicazioni, della schermata di blocco e dei
  banner delle notifiche (vedi [La barra dinamica](#la-barra-dinamica)). A
  riposo è il dock: a sinistra il marchio **J▮** di justOS su una pillola
  scura (un clic apre il launchpad, il tasto destro il menu di sistema:
  *Info su justOS*, *Impostazioni di sistema*, *Negozio*, *Monitor
  attività*, *Desktop e finestre*, *Schermata*, *Blocca schermo*,
  *Sospendi* quando il computer può, *Riavvia*, *Spegni*), poi le app tenute
  nella barra (File, Editor di testo, Note, Calendario, Foto, Musica,
  Calcolatrice, Negozio, Impostazioni), quelle aperte che non ci sono già e
  il Cestino; con una canzone in corso, la sua copertina e un equalizzatore;
  a destra lo stato (rete, volume e, sui portatili, un anello intorno con la
  carica della batteria). Un puntino sotto un'icona segna le app aperte;
  quella di un'app che parte **rimbalza**. Il tasto destro su un'icona apre
  il suo menu (*Apri* o *Mostra*, *Nuova finestra*, le sue finestre,
  *Rimuovi dal dock* / *Mantieni nel dock*, *Chiudi*; per il Cestino *Apri*
  e *Svuota cestino*). Le app tenute stanno nelle impostazioni (`dock`).
- **Centro di controllo** (clic sullo stato): sopra la barra una pillola con
  il nome dell'account e un pulsante con l'ingranaggio delle Impostazioni;
  nella barra i riquadri *Wi-Fi* e *Bluetooth* (viola quando sono collegati;
  la freccia apre la loro pagina con le reti o i dispositivi, l'interruttore
  e *Impostazioni Wi-Fi…*), *Non disturbare*, due cursori verticali per
  volume e luminosità (si trascinano o si gira la rotella), la canzone di
  Musica con riproduci e avanti, *Blocca*, *Riavvia*, *Spegni* e la batteria
  (o *Corrente* senza batteria). Esc o un clic fuori tornano indietro di un
  passo.
- **Widget** (tasto destro → *Mostra i widget*): saluto, calendario del
  mese, stato del sistema.
- **Launchpad** (il marchio J▮ o Ctrl+Spazio): la barra diventa un campo di
  ricerca e sopra compare la griglia di tutte le app, comprese quelle
  installate dal Negozio, a pagine (pallini, frecce, rotella); si scrive e
  Invio apre la prima, Esc chiude. La ricerca è anche quella di **Spotlight**: sopra le app
  compaiono i **calcoli** (`12*3+4` → `= 40`, Invio lo copia), le
  **impostazioni** per parola («sfondo», «volume», «password», «processore»...)
  e i **file e le cartelle** della home per nome, che si aprono con l'app
  giusta. La home viene letta una volta sola all'apertura della ricerca.
  Da tre lettere in su si cerca anche **nel contenuto** dei file di testo
  (`txt`, `md`, `rs`, `c`, `h`, `py`, `json`, `csv`, `html`, `xml`,
  `conf`, `sh`, `log`...): i risultati arrivano sotto quelli per nome, nella
  sezione *Contenuto*, con la riga trovata e le parole cercate più chiare.
  La ricerca va avanti qualche millisecondo per fotogramma, così scrivere
  non aspetta mai: al più 200 file per ricerca, fino a 256 KiB l'uno, i più
  vicini alla home per primi, 3 risultati; i testi letti restano in memoria
  (fino a 4 MiB) finché la ricerca è aperta, così la lettera seguente non
  rilegge il disco. Maiuscole e minuscole, anche accentate, sono uguali.
- **Notifiche**: un'app installata, un timer scaduto, un file salvato, un
  programma che si è chiuso per un errore. La barra diventa la notifica
  (icona, app, titolo, testo e una linea che si consuma) per 4,6 secondi; con
  *Non disturbare* acceso mostra solo *Notifica silenziata*. Restano nel
  centro notifiche (clic sull'orologio).
- **Tasto destro** sulla scrivania: cambiare sfondo, mostrare o
  nascondere i widget, impostazioni dello schermo, informazioni.
- **Finestre**: una cornice chiara smerigliata con il nome dell'app in alto
  a sinistra e, in alto a destra, una pillola con tre pulsanti tondi: riduci
  e schermo intero grigio scuro, chiudi arancione; il contenuto sta su un
  incavo arrotondato. Si spostano trascinando la riga del titolo; il
  **doppio clic** la ingrandisce fino alla barra, che resta visibile (un
  altro doppio clic la riporta com'era). Si ridimensionano da tutti e
  quattro i lati e gli angoli (otto maniglie), fino a 460 × 320 al minimo.
  Ridotta, la finestra si piega a **genio** dentro la sua icona nella barra
  (450 ms) e ne riesce allo stesso modo. A **schermo intero** (il pulsante
  centrale o **F11**) la finestra prende tutto lo schermo e la barra si
  nasconde: torna portando il puntatore sul bordo in basso o premendo da
  solo il tasto logo; Esc o F11 escono. **Alt+Tab** passa alla finestra
  successiva, **Ctrl+Q** chiude quella in primo piano.
- **Dialoghi**, come in macOS e Windows: un dialogo che dice a quale
  finestra appartiene (un selettore di file o una domanda di un programma
  Linux, vedi [compat.md](compat.md)) si apre al centro di quella finestra
  e le resta sempre sopra. Portare davanti la finestra (clic, dock,
  Alt+Tab, vista d'insieme) porta davanti anche i suoi dialoghi, e il
  fuoco resta al dialogo; ridurla li riduce, riaprirla li riporta; spostarla
  su un altro desktop li porta con lei. Nella vista d'insieme un dialogo sta
  sopra il riquadro della sua finestra, e il menu del dock non lo elenca.
- **Snap**, come in Windows: una finestra trascinata sul bordo sinistro o
  destro occupa quella metà dello schermo, in un angolo un quarto, sul bordo
  in alto tutto lo schermo; mentre si trascina un riquadro chiaro mostra
  dove andrà.
  Da tastiera (finestre ridimensionabili) **Ctrl+Alt+←/→** per le metà,
  **Ctrl+Alt+U/I/J/K** per i quarti, **Ctrl+Alt+↑** ingrandisce e
  **Ctrl+Alt+↓** ripristina. Trascinata via, la finestra torna alla sua
  misura di prima.
- **Desktop virtuali e vista d'insieme**: **F3** (o **logo+Tab**, o
  *Desktop e finestre* nel menu J▮) mostra in alto i desktop e sotto
  le finestre di quello attuale affiancate; un clic su una finestra la porta
  davanti, trascinata su un desktop ci si sposta, trascinata sul **+** ne
  crea uno nuovo (fino a sei); la × su un desktop lo toglie e le sue
  finestre passano a quello prima. **Ctrl+Alt+PagSu/PagGiù** (o
  **logo+Ctrl+←/→**) passano al desktop accanto, **Ctrl+Alt+Maiusc+←/→**
  ci portano la finestra in primo piano (oltre l'ultimo ne crea uno). Con
  più desktop, dei pallini sotto l'ora dicono dove si è; il dock e Alt+Tab
  portano al desktop della finestra scelta.
- **Trascinare i file**: da File si trascinano uno o più file (Ctrl+clic e
  Maiusc+clic selezionano, Ctrl+A tutto) su una cartella, su una risorsa a
  sinistra, su una scheda o su un'altra finestra di File. Sullo stesso
  volume si spostano, tra volumi diversi si copiano; tenendo **Ctrl** (o
  Alt) si copia sempre e compare un **+** verde, con **Maiusc** si sposta
  sempre. Rilasciati sul Cestino del dock ci finiscono dentro; sull'icona di
  un'app (Editor, Foto, Musica, File) si aprono lì; sul
  Terminale ne scrivono il percorso; nell'Editor si aprono. Le app del
  Negozio che lo chiedono (Disegno) ricevono i file, solo in lettura.
- **Schermata**: il tasto **Stamp** (o *Schermata* nel menu J▮) salva
  lo schermo in `sys:/home/Immagini` come `Schermata <data> alle <ora>.qoi`,
  che Foto apre subito.
- **Blocco**: **Ctrl+Maiusc+L**, *Blocca schermo* nel menu J▮ o *Blocca* nel
  centro di controllo nascondono le finestre: restano lo sfondo sfocato,
  l'ora grande con la data e la barra, che diventa il blocco con l'iniziale e
  il nome dell'account. Se la password è un **PIN** di 4-8 cifre compare un
  tastierino (anche con i tasti numerici; all'ultima cifra controlla da
  solo); altrimenti un campo per la password e Invio. Un errore scuote i
  pallini in rosso. Con una password (*Impostazioni → Blocco e password*)
  justOS si avvia bloccato, si blocca da solo dopo 2, 5 o 15 minuti senza
  usarlo e prima di ogni sospensione; senza password basta Invio. La
  password non è salvata in chiaro: nelle impostazioni c'è un sale casuale e
  20 000 passaggi di SHA-256 (`settings::check_password`).
- **Aggiornamenti**: con la rete, all'avvio e una volta al giorno justOS
  legge l'indice firmato degli aggiornamenti. Una versione nuova si
  scarica e si scrive nell'altro slot di sistema; con gli aggiornamenti
  automatici succede da sola, poi una notifica chiede di riavviare. In
  *Info sistema* il pulsante passa da *Cerca aggiornamenti* a *Installa*,
  *Scarico…*, *Installo…* e *Riavvia*. Accanto ci sono *Annulla* e *Torna a
  <versione precedente>*. Se la versione nuova non parte, justOS torna da
  solo alla precedente e lo dice. Una release su GitHub che l'indice non ha
  ancora è solo annunciata.

### Lo stile a pixel morbidi

Menu, dialoghi e app sono superfici **opache**, senza sfocatura dello sfondo
né contorni scuri. Fanno eccezione la barra dinamica (vetro scuro, vedi
[La barra dinamica](#la-barra-dinamica)) e la cornice chiara delle finestre.

- **Superfici** (`ui::panel`): una sfumatura dall'alto al basso
  (`panel_top` → `panel_bottom`), una linea sottile tutt'intorno e un
  riflesso chiaro lungo il bordo superiore (`Canvas::sheen_round`); sotto,
  un'ombra morbida (`ui::drop_shadow`). Il raggio è 26 per finestre,
  centro di controllo e centro notifiche, 28 per il dock, 18 per gli incavi e le
  schede (`ui::well_pane`: il colore `well` con una linea sottile).
- **Riquadri** (`ui::tile`): i quadratini che reggono un simbolo (i tre
  pulsanti della finestra, l'ingranaggio, le icone delle sezioni, i
  riquadri del centro di controllo). Hanno il raggio pari a un quarto del
  lato, una sfumatura verticale, il riflesso in alto e una piccola ombra.
  Quello arancione è `ui::ORANGE_GRAD`; il grigio, `ui::knob_grad()`.
- **Simboli a pixel morbidi**: ogni simbolo è poche righe di testo (`#` è
  una cella piena, `.` vuota, qualsiasi altro carattere una cella con un
  colore a parte), disegnato da `Canvas::soft_grid` tramite
  `ui::soft_pixels_at`, `soft_pixels_in` (centrato in un rettangolo) o
  `soft_pixels_keyed`. La regola degli angoli: un angolo **in fuori** si
  arrotonda di mezza cella; dove due celle piene si incontrano intorno a
  una vuota, l'angolo **in dentro** si raccorda con una curva di 0,38
  celle. La cella si dà in millesimi di unità, così lo stesso simbolo vale
  a ogni scala; l'angolo in alto a sinistra si allinea a un pixel intero,
  quindi celle di un numero intero di pixel escono nitide. Sono fatti così
  i simboli del dock e del centro di controllo, quelli dei pulsanti della
  finestra e delle sezioni delle Impostazioni.
- **Cursori a celle** (`ui::cell_bar`, `ui::slider`): una fila di 16 celle
  arrotondate che si riempiono da sinistra; piene in arancione (volume,
  accento) o nel colore del testo (luminosità).
- **Orologio**: le cifre sono pixel morbidi di 3×5 celle (`DIGITS` in
  `desktop.rs`, cella da 10 unità), con la data sotto nel carattere
  normale. Il colore è bianco o nero secondo la luminosità dello sfondo
  sotto l'orologio, con un'ombra leggera fatta ridisegnando le cifre un
  po' più in basso.

### Il tema scuro e chiaro

Ci sono due tavolozze, scura e chiara (`ui::DARK` e `ui::LIGHT`), e si
sceglie in *Impostazioni → Aspetto → Tema*. La scelta è nelle impostazioni
come `theme=dark` o `theme=light` (`settings.rs` la legge all'avvio e la
scrive a ogni salvataggio) e `ui::set_theme` la rende attiva; senza
scelta il tema è scuro.

I colori **non sono costanti**: si leggono con le funzioni di `ui`, che
guardano la tavolozza del tema in uso: `ui::text()`, `text_dim()`,
`text_faint()`, `well()`, `pane()`, `line()`, `sheen()`, `sel()`,
`hover()`, `cell()`, `panel_top()`, `panel_bottom()`, `knob_grad()`,
`knob_glyph()` e `shadow_strength()` per le ombre (più deboli nel chiaro).
Il tema attivo si legge con `ui::theme()`. Cambiando tema, Impostazioni
restituisce `Action::RedrawAll` e la scrivania ridisegna tutto lo schermo.

Come sceglie i colori un'app:

- testo, linee, selezioni, passaggio del puntatore e campi vanno sempre
  presi da queste funzioni, mai scritti come bianco o nero con
  trasparenza, perché nell'altro tema non si leggerebbero;
- vanno bene colori fissi solo per ciò che è uguale in entrambi i temi:
  l'arancione (`ui::ORANGE`, `ORANGE_GRAD`), il rosso e il verde di stato,
  i riquadri colorati, le immagini dell'app. Se un colore deve essere più
  scuro nel tema chiaro (il rosso dei festivi in Calendario) l'app guarda
  `ui::theme()` e ne ha due;
- i componenti comuni (`ui::button`, `toggle`, `slider`, `segmented`,
  `search_field`, `text_field`) e le superfici (`panel`, `well_pane`,
  `tile`) seguono già il tema.

### Le Impostazioni

A sinistra una barra con la ricerca e otto sezioni, ognuna con il suo
riquadro colorato e un simbolo: *Generale*, *Aspetto*, *Schermo*, *Suono*,
*Bluetooth*, *Tastiera*, *Blocco e password*, *Privacy*. A destra le **schede**
(`Card`) della sezione, ognuna con più **righe** (`Row`): un'etichetta, una
riga di spiegazione facoltativa e un controllo (`Ctl`) a destra, oppure
sotto per quelli larghi (sfondi, risoluzioni). Il tema si sceglie da due
anteprime disegnate, scuro e chiaro. Le frecce su e giù cambiano sezione,
Pag su e Pag giù scorrono.

La sezione *Privacy* ha una scheda con i permessi (fotocamera, microfono,
cartelle…): un clic apre l'elenco delle app che lo hanno chiesto, ognuna
con un interruttore. Sotto c'è una scheda per app, con un interruttore per
permesso e «Chiedi di nuovo», che dimentica le sue risposte. Le app del
Negozio installate compaiono con i permessi del manifest anche prima di
averli chiesti. La finestra della domanda è in `gui/apps/prompt.rs`.

La sezione *Bluetooth* ha l'adattatore con il suo interruttore, «Cerca»
(otto secondi, BR/EDR e LE insieme) e i dispositivi: quelli associati
con «Collega» o «Scollega» e «Dimentica», quelli trovati con «Associa».
Ogni azione va in un thread suo e la riga dice cosa sta facendo e come è
finita. Quando un'associazione ha bisogno dell'utente, da qualunque parte
sia partita (le Impostazioni, `bt pair`, un'app), si apre una finestra
come quella dei permessi (`gui/apps/btpair.rs`): il codice da confrontare
con «Associa» e «Annulla», il codice da scrivere sulla tastiera, o un
campo per quello che mostra il dispositivo.

La **ricerca** si apre scrivendo (Esc la chiude): ignora maiuscole e
accenti (`fold`), cerca nelle etichette delle righe e nei nomi delle
sezioni e mostra i risultati raggruppati per sezione. Le frecce scelgono un
risultato; Invio o un clic aprono la sezione e accendono per un momento la
riga trovata.

Per **aggiungere una riga** si modifica `Prefs::cards` in
`system/kernel/src/gui/apps/prefs.rs`: nella sezione giusta si aggiunge
`row("Etichetta", "Spiegazione", Ctl::…)` a una scheda (o una nuova
`card(vec![...])`). Va bene un controllo che già c'è (`Ctl::Info` per un
valore, `Ctl::Text` per una riga di testo, `Ctl::Themes`, `Ctl::Layout`…).
La ricerca la trova da sola, perché cerca nelle righe di `cards`. Un
controllo nuovo vuole una variante di `Ctl` e il suo disegno in
`draw_ctl`, la misura in `ctl_size`, le zone cliccabili in `parts` e
l'azione in `act`. Se cambia qualcosa dei colori, `act` restituisce
`Action::RedrawAll`. Una sezione nuova si aggiunge a `Section` e a
`SECTIONS` con nome, sfumatura e simbolo.

### La barra dinamica

La scrivania ha una sola superficie di sistema: la **barra dinamica**, in
basso al centro dello schermo principale, a 28 unità dal bordo. Non ci sono
altri pannelli: quando succede qualcosa la barra **cambia forma** (larghezza,
altezza e raggio degli angoli, 520 ms con un leggero rimbalzo) e il suo
contenuto si scambia (quello che esce sbiadisce in 200 ms e si rimpicciolisce
del 3%, quello che entra aspetta 80 ms e cresce fino alla misura piena).
Disegno e misure vengono da `docs/design/dynamic-bar/` (il documento e la
demo `reference/dynamic-bar-demo.html`, che vale dove i due non sono
d'accordo), con una scelta nostra: al posto del logo della demo ci sono il
marchio e la scritta di justOS.

| Stato | Forma (unità a 1440 × 1024) | Cosa mostra |
|---|---|---|
| a riposo | larga quanto serve × 68 | il dock (vedi [La scrivania](#la-scrivania)) |
| launchpad | 540 × 60 | la ricerca; sopra, la griglia delle app su un velo sfocato |
| blocco | 340 × 540, centrata sotto l'ora | iniziale, nome, PIN o password |
| centro di controllo | 712 × 316, con la pillola dell'account sopra | i riquadri; Wi-Fi e Bluetooth hanno una pagina loro (440 × 384) |
| volume, luminosità | 400 × 68, 1,6 s | il simbolo, la barra e il valore |
| notifica | 490 × 92, 4,6 s | l'icona e il nome dell'app, titolo, testo |
| non disturbare | 300 × 56, 1,6 s | *Notifica silenziata* |
| musica | 620 × 196 (clic sulla canzone nella barra) | copertina, titolo, artista, avanzamento trascinabile, casuale, indietro, riproduci, avanti, ripeti |
| spegnimento | 420 × 180, poi 72 × 72 e un punto di 14 | *Spegnere il computer?* con *Annulla* e *Spegni*; poi una rotellina, la barra che diventa scura e lo schermo nero |
| carica | 330 × 68, 2,6 s | *In carica* e la percentuale quando si attacca l'alimentatore |

Come si comporta:

- Gli stati e le forme sono in `system/libs/justshell/src/bar.rs` (`Bar`,
  `BarState`), con le prove sul computer di sviluppo (`make check`): le
  forme, le durate degli stati che tornano da soli al dock, i passaggi
  vietati (dal blocco si esce solo sbloccando), la barra nascosta a schermo
  intero. Il disegno e i clic sono in
  `system/kernel/src/gui/desktop/shell.rs`.
- Sotto il centro di controllo, la musica e lo spegnimento la scrivania si
  scurisce; il launchpad, il blocco e lo spegnimento nascondono le finestre.
- Il vetro: dove sotto c'è solo lo sfondo, la barra mostra lo sfondo sfocato
  con una tinta scura; dove passa sopra una finestra, che non ha una copia
  sfocata, diventa scura opaca (il colore rgba(18, 18, 22, 0,85) reso opaco). Non c'è
  un vero `backdrop-filter`.
- I dati sono veri: Wi-Fi (`net::wifi`: stato, reti, connessione a una rete
  aperta o già salvata), Bluetooth (`bt`: acceso o spento, dispositivi),
  batteria (`acpi::power`), volume e luminosità (gli stessi di tastiera e
  Impostazioni), la canzone dell'app Musica (titolo, artista, durata,
  comandi, attraverso `App::media`).
- Si ridisegna solo quando qualcosa si muove: a riposo nessun fotogramma.
  Quando un movimento finisce si disegna ancora un fotogramma completo, così
  anche un fotogramma lento (QEMU senza accelerazione) non lascia la barra a
  metà strada.
- Dalla shell `notify <titolo> <testo> [icona]` manda una notifica per
  provarla.
- Rispetto al dock di prima mancano le capsule dei programmi che si avviano
  e del timer (il programma che parte ha la sua icona che rimbalza, la sua
  nota arriva come notifica), i separatori e le etichette delle icone, e le
  impostazioni *Dock compatto* e *Ingrandimento icone*.

### Le animazioni

Tutto ciò che si muove segue il tempo, non i fotogrammi: su un computer lento
si vedono meno fotogrammi ma l'animazione dura uguale.

| Cosa | Animazione |
|---|---|
| finestra che si apre | cresce dal 86% e compare in dissolvenza |
| finestra che si chiude | si rimpicciolisce e svanisce |
| riduci / riapri | la finestra si piega a genio nella sua icona della barra e ne esce (450 ms) |
| ingrandisci / ripristina | la finestra si allarga fino alla barra (o a tutto lo schermo) e torna |
| launchpad | lo sfondo si sfoca sotto un velo, la barra diventa la ricerca |
| dock | rimbalzo dell'icona all'avvio di un'app |
| barra dinamica | cambia forma in 520 ms con un leggero rimbalzo, il contenuto si scambia in dissolvenza; a schermo intero scende sotto il bordo e risale |
| menu | si aprono in un attimo, dalla barra verso l'alto |
| notifiche | la barra diventa la notifica e poi torna il dock |
| vista d'insieme | le finestre scivolano dal loro posto alla griglia, i desktop scendono dall'alto |
| cambio di desktop | il nome del desktop e i suoi pallini al centro, per un attimo |
| spegnimento | lo schermo sfuma nel nero con il logo |

Una finestra in movimento viene disegnata una volta in un'immagine a parte
(con la sua trasparenza) e poi solo spostata, scalata e sfumata: l'app non
si ridisegna a ogni fotogramma.

### Fluidità e fotogrammi

Il ciclo dell'interfaccia (`Looper::run` in `system/kernel/src/gui/ui.rs`,
con il passo in `system/kernel/src/gui/pace.rs`) disegna solo la parte dello
schermo che è cambiata, e **al massimo un fotogramma per periodo**: mentre
qualcosa si muove (un'animazione, il puntatore) disegna al ritmo scelto,
puntatore compreso; quando niente si muove non disegna niente. Il tick del
sistema resta a 1 kHz: il ciclo dorme fino al millisecondo giusto, ma le
scadenze sono tenute in nanosecondi (`time::uptime_ns`), così in media i
fotogrammi arrivano al ritmo scelto (a 240 Hz uno ogni 4 o 5 ms).

A riposo il ciclo dorme. Lo svegliano subito il mouse e la tastiera
(`input::wake_gui`) e la finestra di un programma che manda un'immagine
nuova (`window::present`). Entro un millisecondo lo svegliano anche le
finestre dei programmi Linux che hanno disegnato (il compositor Wayland), il
testo nuovo del Terminale (che lo mostra al fotogramma successivo, non al
`Tick`), le finestre nuove e le richieste di altri thread (risoluzione,
dimensione dell'interfaccia, Impostazioni, programmi Windows pronti): chi ha
qualcosa da mostrare chiama `input::wake_gui_soon`, che è un solo flag senza
lock, e il timer di sistema sveglia il ciclo al tick successivo. Ogni 100 ms il ciclo
manda alle app il loro `Tick` e intanto dà un'occhiata a tutto il resto, come
rete di sicurezza: a scrivania ferma si sveglia una decina di volte al
secondo, non più 125.

*Impostazioni → Schermo → Fluidità* sceglie il ritmo massimo: *Automatica*,
*60 Hz*, *120 Hz*, *144 Hz*, *240 Hz*. *Automatica* sarebbe la frequenza
dello schermo, ma oggi nessun driver la conosce (l'interfaccia video non la
porta e nessuno legge l'EDID), quindi vale 60 Hz. La scelta resta nelle
impostazioni (`fluidity`).

Dal Terminale (o dalla console seriale) `fps` dice come è andato l'ultimo
secondo; `fps 240` (o `60`, `120`, `144`, `auto`) cambia anche il ritmo:

```
cap 240 Hz (chosen)
last second: 239 frames, 0.0 ms each on average, 0.1 ms at most; longest gap 5.0 ms
loop: 253 wake-ups in the last whole second
```

*frames* sono i fotogrammi mostrati (anche quelli in cui si è mosso solo il
puntatore), *each* quanto ci è voluto a disegnarli, *longest gap* la pausa
più lunga tra due, *wake-ups* quante volte il ciclo si è svegliato. In QEMU
senza KVM il numero di fotogrammi è limitato dal processore emulato:
spostare una finestra costa decine di millisecondi a fotogramma, il solo
puntatore quasi niente.

## Le app di sistema

| App | Cosa fa |
|---|---|
| File | sfoglia i volumi: risorse a sinistra, cartelle e file a destra; doppio clic apre una cartella o il file con l'app giusta; **schede** (Ctrl+T o il **+**, Ctrl+W chiude, Ctrl+Tab passa alla successiva, *Apri in una nuova scheda* sulle cartelle); tasto destro per aprire, rinominare (anche F2), spostare nel Cestino (Canc), creare una cartella; nel Cestino eliminare o svuotare; **selezione multipla** (Ctrl+clic, Maiusc+clic, Ctrl+A) e **trascinamento** tra cartelle, schede e finestre; le immagini (QOI, PNG, JPEG, GIF) mostrano la loro **miniatura** al posto dell'icona, fatta da un thread a parte (`gui/thumbs.rs`) mentre si guarda: prima quelle in vista, l'icona finché non è pronta e per sempre per i file oltre 20 MiB, le foto oltre 16 megapixel e quelli che non si leggono; le miniature (fino a 8 MiB) si ricordano per percorso, dimensione e data di modifica |
| Editor di testo | file di testo con selezione, copia e incolla, scorrimento; Nuovo, Apri, Salva (Ctrl+N, Ctrl+O, Ctrl+S) con la scelta del file (salvando, il nome arriva con «Senza titolo» già selezionato: si scrive il nome e l'estensione resta); alla chiusura salva da solo |
| Note | note in `sys:/home/Note`, la prima riga è il titolo, ricerca, si salvano mentre si scrive |
| Promemoria | cose da fare, spuntate con un clic (`sys:/home/Promemoria.txt`) |
| Calendario | un mese alla volta con le feste italiane (anche Pasqua e Pasquetta) e i propri eventi («9:30 Riunione»), salvati in `sys:/home/Calendario.txt` |
| Orologio | l'ora qui e in altre città, cronometro con i giri, timer che suona e avvisa |
| Calcolatrice | come quella di macOS, anche da tastiera; calcoli in virgola fissa con dodici decimali |
| Foto | le immagini di `Immagini` e gli sfondi, in miniatura e a tutto schermo |
| Musica | i file WAV di `Musica` e i suoni di sistema: riproduzione, pausa, brano precedente e successivo, barra di avanzamento, volume; tre brani di esempio |
| Anteprima | un'immagine sola (QOI, PNG, JPEG, GIF), aperta da File |
| Negozio | le app di `sys:/store` per categoria, con la loro pagina; installa, apre, rimuove |
| Impostazioni | otto sezioni con ricerca: nome e configurazione iniziale, tema e sfondo, risoluzione e dimensione dello schermo, volume e suono all'avvio, Bluetooth, tastiera, password e blocco, permessi delle app (vedi [Le Impostazioni](#le-impostazioni)) |
| Monitor attività | i programmi in esecuzione, il processore e la memoria nell'ultimo minuto; un programma si può terminare |
| Info sistema | panoramica, dispositivi con i loro driver, dischi con lo spazio libero |
| Terminale | la shell di justOS: l'unico posto dove si vedono le righe di comando; `help` elenca i comandi e `help <comando>` ne spiega uno, in italiano; i nomi dei comandi e le loro parole chiave (`drv list`, `sysupdate check`) restano in inglese |

## Il Negozio e le app installabili

Il Negozio installa programmi veri, separati dal sistema: ognuno gira in
modalità utente nel suo spazio di indirizzi e apre una finestra sulla
scrivania con le chiamate di sistema `WIN_*` (vedi
[programmi](programmi.md#programmi-con-una-finestra)). Ce ne sono sei:

| App | Categoria | |
|---|---|---|
| 2048 | Giochi | le tessere da unire, con animazioni e record |
| Campo minato | Giochi | tre livelli, bandierine con il tasto destro |
| Serpente | Giochi | sempre più veloce, record salvato |
| Sudoku | Giochi | partite sempre nuove con una sola soluzione, matita, errori in rosso |
| Tris | Giochi | contro il computer (in *Difficile* non perde mai) |
| Disegno | Creatività | matita, pennello, gomma, forme, secchiello, colori; salva in `Immagini` |

e due programmi per il Terminale, BusyBox e Lua (Linux). Ogni pacchetto è una
cartella `sys:/store/<id>` con il programma (`app`), l'icona (`icon.qoi`) e
una scheda (`info`: nome, categoria, descrizione, versione, tipo). Installare
copia questi file in `sys:/apps`; da lì le app compaiono in Applicazioni. Dal
Terminale: `install`, `install <nome>`, `uninstall <nome>`.

## Schermo e suono

Con un driver video (`bochsvga`, `vmsvga`, `virtiogpu`: le schede delle
macchine virtuali) *Impostazioni → Schermo* elenca le risoluzioni comuni
che lo schermo offre; un clic la cambia subito, e justOS la riprende a ogni
avvio. Senza driver resta la risoluzione scelta dal firmware. Dal
Terminale: `display` elenca, `display 1920x1080` cambia.

Sugli schermi ad alta densità l'interfaccia raddoppia: tutto è misurato in
*unità*, e da 2560×1440 in su (se restano almeno 800×600 unità) ogni unità
è di 2×2 pixel. Il testo non viene ingrandito ma disegnato da un secondo
atlante di glifi fatto al doppio della misura (`fonts@2x.bin`), le icone e
gli sfondi sono ridotti dagli originali grandi, il puntatore è ridisegnato
alla nuova misura: stessa disposizione, il doppio dei dettagli. *Impostazioni
→ Schermo* (quando lo schermo è abbastanza grande) sceglie tra *Auto*,
*100%* e *200%*; dal Terminale `display scale auto|1|2`. La scelta resta
nelle impostazioni.

### Più monitor

Con più monitor (oggi `virtiogpu`, in QEMU `-device
virtio-gpu-pci,max_outputs=2`) lo schermo è uno solo, diviso fra i monitor
messi uno accanto all'altro, tutti alla stessa risoluzione e alla stessa
scala (la risoluzione scelta è quella di ogni monitor). Uno è il
*principale*: lì stanno l'orologio e la barra dinamica, e lì si aprono il
launchpad, la vista d'insieme, le notifiche, i menu della barra e il centro
di controllo. Lo sfondo copre ogni monitor, scalato alla sua
misura; il blocco schermo li copre tutti (l'ora e l'account sul
principale).

Una finestra appartiene al monitor che ha il suo centro: lì si massimizza
(e a tutto schermo riempie quel monitor soltanto; la barra si nasconde solo
se il monitor è il principale), lì si aggancia a metà o a un quarto, e il
bordo fra due monitor non aggancia: il puntatore lo attraversa e la
finestra trascinata passa all'altro. Una finestra nuova si apre sul
monitor del puntatore, un dialogo su quello della sua finestra.

*Impostazioni → Schermo → Monitor* (solo con più monitor collegati) li
disegna come stanno, con lo sfondo e un numero, l'orologio e il dock sul
principale: un clic su un monitor lo rende principale, *Scambia* inverte
destra e sinistra, *Duplica* fa mostrare a tutti la stessa scrivania. Le
scelte restano nelle impostazioni (`monitors=`, `primary=`, `mirror=`).
Dal Terminale: `display` elenca i monitor, `display primary N`,
`display order N,N` e `display mirror on|off` (N è il numero
dell'uscita). Un monitor collegato o staccato mentre justOS gira fa
ridisporre lo schermo da sé: le finestre restano sul loro monitor, o vanno
sul più vicino se il loro se n'è andato.

Fino alla scrivania (avvio e configurazione iniziale) i monitor mostrano
tutti la stessa immagine. Wayland annuncia un `wl_output` per monitor, il
principale per primo.

Con una scheda audio (`hda`, `ac97`) la scrivania suona una breve melodia
quando compare (si può spegnere), il volume si regola dal centro di
controllo e dalle Impostazioni, e Musica e Orologio suonano. Dal Terminale:
`play file.wav`, `tone 440 500`, `volume 60`. Senza scheda audio il volume
nel centro di controllo è spento e grigio e non si sposta, l'altoparlante
nella barra è sbiadito e *Impostazioni → Suono* dice che manca la scheda.

## Com'è fatta

L'interfaccia è nel kernel, in `system/kernel/src/gui`; il disegno è una
libreria condivisa con i programmi, `system/libs/justgfx`. Niente librerie
esterne.

| File | Contenuto |
|---|---|
| `justgfx` (`gfx.rs`, `font.rs`, `qoi.rs`) | superfici e disegno: rettangoli arrotondati antialiasati, il riflesso sul bordo (`sheen_round`), i pixel morbidi (`soft_grid`), trasparenze (anche su livelli trasparenti), sfumature, ombre, sfocatura, linee, archi, ridimensionamento; il testo dall'atlante dei glifi; le immagini QOI (lettura e scrittura). Solo aritmetica intera |
| `text.rs`, `res.rs` | i caratteri e le icone di sistema, gli sfondi |
| `thumbs.rs` | le miniature delle immagini per File: un thread che legge a pezzi, decodifica e rimpicciolisce, una cache limitata in byte e in voci |
| `splash.rs`, `setup.rs`, `desktop.rs` | avvio, configurazione guidata, scrivania, finestre e animazioni; `desktop/shell.rs`: la barra dinamica |
| `wm.rs` | le finestre e l'interfaccia `App` delle app |
| `window.rs` | le finestre dei programmi in modalità utente |
| `ui.rs` | il tema e le sue tavolozze, superfici (pannelli, incavi, riquadri), pixel morbidi, componenti comuni (pulsanti, cursori a celle, interruttori, schede), animazioni (`Anim`, curve di accelerazione), appunti, il ciclo degli eventi |
| `textarea.rs` | il testo su più righe di Editor, Note |
| `apps/` | le app di sistema |

Il disegno avviene in un buffer in memoria; sullo schermo si copia solo la
parte che cambia. Un'app può dire quale parte del suo contenuto è cambiata
(`App::changed`): il cursore che lampeggia nel Terminale o nell'Editor
ridisegna una cella, non la finestra, e l'output del Terminale solo le
righe nuove. Il puntatore è disegnato direttamente sullo schermo, così
muoverlo non ridisegna nulla. Una copia sfocata dello sfondo, calcolata una
volta sola, serve dietro Applicazioni, la vista d'insieme e il blocco. I clic del mouse vengono
messi in coda dal driver, così un clic rapido non si perde anche se la
scrivania sta disegnando un'animazione.

La console di testo continua a esistere: con la grafica attiva è una griglia di
80x25 caratteri che la finestra Terminale disegna, e la tastiera passa alla
shell solo quando il Terminale è in primo piano.

## Le risorse

Caratteri, icone, sfondi e suoni sono generati in fase di sviluppo da script
Python (`tools/assets/`) e salvati nel repository; per ricostruire justOS non
servono.

- `gen_fonts.py` → `system/kernel/assets/fonts.bin`: Inter e DejaVu Sans
  Mono (licenze in `system/kernel/assets/LICENSES.md`); una copia va in
  `sys:/system/fonts.bin` per i programmi. Con `--sharp` gli stessi
  caratteri al doppio della misura (`fonts@2x.bin.gz`, per gli schermi ad
  alta densità; i programmi lo trovano in `sys:/system/fonts@2x.bin`).
- `gen_pixicons.py` → `rootfs/system/gui/icons.qoi`: le icone a pixel di
  justOS, quasi novanta, su una griglia: app in un riquadro di 20×20 celle
  con la sfumatura, glifi di sistema in 12×12, angoli esterni arrotondati
  di mezza cella e raccordi di 0,38 negli angoli interni (`pixgrid.py`).
  Le icone di partenza sono gli SVG di `tools/assets/src/icone-justos`
  (letti cella per cella); le altre sono disegnate nello script come
  testo, un carattere per cella. Con `--store` le icone dei giochi del
  Negozio. Basta Pillow.
- `gen_pixel.py` → `rootfs/system/gui/wallpapers/pixel.qoi`: lo sfondo
  predefinito, le Dolomiti al tramonto in grandi pixel con il retino
  ordinato (Bayer) nei colori caldi di justOS; basta Pillow.
- `gen_wallpapers.py` → `rootfs/system/gui/wallpapers/`: due sfondi
  (Tramonto, Notte) dipinti con rumore e sfumature.
- `gen_sounds.py` → `rootfs/system/sounds/avvio.wav`, il suono d'avvio, e i
  tre brani di `rootfs/home/Musica`, composti e sintetizzati dallo script.

`make assets` li rigenera (servono Python 3, Pillow e numpy).

## Provarla

`make run` apre justOS in una finestra di QEMU. `make shots` avvia justOS senza
schermo, lo guida con mouse e tastiera (`scripts/gui-shots.py`, attraverso
QMP) lungo `scripts/gui/tour.txt` (i dieci passi della configurazione iniziale, ogni app, il Negozio che
installa e avvia un gioco) e salva le schermate in `build/shots/`; con
`SCREEN=1920x1080` le coordinate del giro si riferiscono a uno schermo di
quella misura. Con `HEADS=2` (e `-vga none -device
virtio-gpu-pci,max_outputs=2,id=gpu`) ogni schermata salva anche ogni
monitor a parte, e i passi `plug`/`unplug` collegano e staccano il secondo
monitor; `SCREEN` è allora tutto lo schermo (due 1280×800: `2560x800`).
