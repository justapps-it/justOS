# CLAUDE.md: dynamic bar e window manager per justOS

Questo file spiega come portare in **justOS** (sistema operativo scritto interamente in Rust) due cose che esistono già come demo web:

1. **La dynamic bar**: una sola barra in basso al centro che si trasforma (morph) in tutte le superfici di sistema (dock, centro di controllo, sblocco PIN, HUD volume/luminosità, notifiche, player musicale, launchpad con ricerca, spegnimento, ricarica).
2. **Le finestre delle app**: cornice chiara smerigliata, ridimensionabili, con riduci a icona "genie", massimizza con doppio clic sulla barra del titolo e **schermo intero stile macOS** dal pulsante, con la barra che si nasconde e ricompare col tasto Win/Super o portando il mouse in basso.

La demo è la **fonte di verità** per misure, tempi, curve e comportamento: `reference/dynamic-bar-demo.html` (apri `reference/` con un server statico qualsiasi, es. `python3 -m http.server`, e vai su `dynamic-bar-demo.html`). Se questo documento e la demo non coincidono, vince la demo.

---

## 0. Prima di scrivere codice

Non conosco l'interno di justOS. Quindi **prima esplora il repo** e adattati a quello che c'è, non inventare API:

- Trova il **compositor / display server**: chi possiede le surface, lo z-order, il damage tracking, il vsync/frame clock.
- Trova lo **strato di rendering 2D**: rettangoli arrotondati, blur dello sfondo (backdrop), ombre, gradienti, testo, SVG/path. Se il blur non c'è, usa il fallback descritto in §9.
- Trova il **sistema di input**: eventi puntatore (move/down/up, con coordinate globali), tastiera (keydown/keyup con il tasto Super), focus.
- Trova se esiste già un **sistema di animazione** (tween, spring, timeline). Se esiste, usalo e aggiungi solo la curva cubic-bezier di §3.
- Trova come si organizzano i crate (`crates/`, workspace `Cargo.toml`), il logging, i test, le convenzioni di naming e di errore.
- Leggi eventuali `README`, `ARCHITECTURE.md`, `docs/` e i CLAUDE.md esistenti: quelli hanno la precedenza su questo file per stile e struttura.

Poi proponi all'utente un piano breve (quali crate tocchi o crei, dove vive lo stato della barra, come passa l'input) **prima** di implementare.

Struttura suggerita, solo se il repo non ne impone una diversa:

```
crates/
  shell-bar/        # state machine della barra + layout + animazioni + rendering delle viste
  shell-wm/         # finestre: geometria, stati, drag/resize, genie, fullscreen, peek della barra
  shell-anim/       # cubic-bezier, tween, (se non esiste già nel repo)
  shell-theme/      # token colori, raggi, font
```

Tutta la logica (state machine, geometria, timer, hit-test) deve essere **pura e testabile senza GPU**: il rendering legge lo stato, non lo possiede.

---

## 1. Coordinate, scala e posizione della barra

- Unità: **pixel logici** (indipendenti dal DPI). Converti in pixel fisici solo nel renderer.
- La barra è ancorata **in basso al centro**, con l'origine della trasformazione al centro del bordo inferiore (`transform-origin: 50% 100%`).
- Distanza dal fondo: **28 px** (salvo i casi sotto).
- Fattore di scala globale `K` (rimpicciolisce tutto su schermi piccoli, mai ingrandisce):

```rust
let k = 1.0_f32.min((screen_w - 32.0) / 880.0).min((screen_h - 60.0) / 640.0);
```

- Stato `Lock`: la barra sta sotto l'orologio grande. `clock_bottom = H*0.07 + min(120, H*0.14) + 40`, `avail = H - clock_bottom - 24`, `k = min(k, avail / h)`, `bottom = max(24, (avail - h*k)/2 + 24)`.
- Stati `Off`, `Boot`, `Spin`: la barra è **centrata verticalmente**: `bottom = max(28, (H - h*k)/2)`.
- La posizione `bottom` si anima in **0.6 s** con `EASE`.

---

## 2. State machine della barra

Una sola surface, un solo stato attivo alla volta. Ogni stato ha `[larghezza, altezza, raggio]` in px logici (prima della scala `K`):

| Stato | W | H | Raggio | Note |
|---|---|---|---|---|
| `Idle` | 690 (760 se la musica è attiva) | 68 | 20 | dock: logo, app, mini player, pill batteria/wifi |
| `Launch` | 540 | 60 | 30 | la barra diventa il campo di ricerca del launchpad |
| `Lock` | 340 | 540 | 32 | tastierino PIN |
| `Control` | 712 | 316 | 28 | centro di controllo |
| `CcWifi` | 440 | 384 | 28 | sottopagina Wi-Fi del centro di controllo |
| `CcBt` | 440 | 384 | 28 | sottopagina Bluetooth |
| `Hud` | 400 | 68 | 34 | volume o luminosità, **transitorio 1600 ms** |
| `Notify` | 490 | 92 | 24 | notifica, **transitorio 4600 ms** |
| `Silent` | 300 | 56 | 28 | "Non disturbare" attivo, **transitorio 1600 ms** |
| `Music` | 620 | 196 | 28 | player espanso |
| `Power` | 420 | 180 | 28 | conferma spegnimento/riavvio |
| `Spin` | 72 | 72 | 36 | cerchio con spinner (spegnimento in corso), sfondo scuro |
| `Boot` | 150 | 96 | 48 | logo all'avvio, sfondo scuro |
| `Off` | 14 | 14 | 7 | puntino, schermo nero |
| `Charge` | 330 | 68 | 34 | animazione ricarica, **transitorio 2600 ms** |

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarState { Idle, Launch, Lock, Control, CcWifi, CcBt, Hud, Notify, Silent, Music, Power, Spin, Boot, Off, Charge }

pub struct Shape { pub w: f32, pub h: f32, pub r: f32 }

impl BarState {
    pub fn shape(self, music_active: bool) -> Shape {
        use BarState::*;
        let (w, h, r) = match self {
            Idle => (if music_active { 760.0 } else { 690.0 }, 68.0, 20.0),
            Launch => (540.0, 60.0, 30.0), Lock => (340.0, 540.0, 32.0),
            Control => (712.0, 316.0, 28.0), CcWifi | CcBt => (440.0, 384.0, 28.0),
            Hud => (400.0, 68.0, 34.0), Notify => (490.0, 92.0, 24.0), Silent => (300.0, 56.0, 28.0),
            Music => (620.0, 196.0, 28.0), Power => (420.0, 180.0, 28.0), Spin => (72.0, 72.0, 36.0),
            Boot => (150.0, 96.0, 48.0), Off => (14.0, 14.0, 7.0), Charge => (330.0, 68.0, 34.0),
        };
        Shape { w, h, r }
    }
    pub fn is_cc(self) -> bool { matches!(self, Self::Control | Self::CcWifi | Self::CcBt) }
    pub fn is_dark(self) -> bool { matches!(self, Self::Spin | Self::Boot | Self::Off) }
    pub fn dims_desktop(self) -> bool { self.is_cc() || matches!(self, Self::Music | Self::Power) }
}
```

### `go(state)`

Annulla il timer del transitorio, imposta il nuovo stato, avvia il morph verso la nuova shape, ricalcola `place()`, accende la vista del nuovo stato e spegne le altre. Effetti collaterali:

- `is_dark()`: sfondo pieno `#0b0b0f` invece del vetro.
- `Music`: vetro tinto col colore della copertina (20% accento sopra `rgba(16,16,20,.8)`).
- `dims_desktop()`: oscura lo sfondo dietro.
- Le intestazioni del centro di controllo (nome, foto, icona impostazioni) stanno **fuori dalla box**, sopra la barra, e sono visibili solo quando `is_cc()`.
- `Launch`: mostra la griglia del launchpad a schermo, nasconde le finestre.

### `transient(state, ms) -> bool` (regole di priorità)

```rust
pub fn transient(&mut self, s: BarState, ms: u32) -> bool {
    use BarState::*;
    if matches!(self.state, Lock | Off | Boot | Spin | Power | Launch) { return false; }
    if self.state.is_cc() && s == Hud { return false; } // nel CC i cursori sono già visibili
    self.go(s);
    self.timer = Some(Timer::after(ms, BarEvent::BackToIdle));
    true
}
```

- Volume/luminosità (tasti o frecce) dentro il centro di controllo: cambia il valore **senza** HUD.
- Notifica con "Non disturbare" attivo: `transient(Silent, 1600)` invece di `Notify`.
- Clic sulla notifica: `go(Idle)`.
- La barra di avanzamento della notifica dura 4.5 s.

### Flussi

- **Avvio**: `Lock`. PIN di 4 cifre. Corretto: i puntini diventano verdi, poi `Idle` dopo 450 ms. Sbagliato: i puntini diventano rossi e tremano, e si azzerano dopo 550 ms. Il PIN `1234` della demo è solo un segnaposto: in justOS va verificato dal servizio di autenticazione del sistema, mai in chiaro nel codice della shell.
- **Spegni**: `Power` → conferma → `Spin` (schermo semi-oscurato) → dopo 1600 ms `Off` (schermo nero). Clic su `Off` → `Boot` → dopo 2200 ms `Lock`.
- **Riavvia**: `Spin`, poi a 1500 ms `Boot`, poi a 3600 ms `Lock`.
- **Ricarica**: `transient(Charge, 2600)` con la cella che si riempie fino alla percentuale attuale.
- **Launchpad**: clic sul logo → `Launch`. La barra diventa un campo di ricerca e la griglia delle app filtra mentre scrivi. Le colonne valgono 7, 6, 5 o 4 in base alla larghezza (`W = min(1080, screen_w*0.92)`: >980 → 7, >800 → 6, >620 → 5, altrimenti 4). Le righe sono 4 se `H > 840`, altrimenti 3. Si sfoglia per pagine con le frecce e Invio apre la prima app. Esc cancella la ricerca, oppure chiude se è già vuota.
- **Scorciatoie tastiera (stato Idle)**:
  - ↑/↓ volume ±5%, →/← luminosità ±5%, `M` muto (tutti con HUD)
  - `N` notifica, `P` play/pausa musica, `C` centro di controllo, `L` blocca, `A` launchpad
  - Esc: torna indietro di un livello (`CcWifi`/`CcBt` → `Control` → `Idle`)

---

## 3. Animazione

### Curve

```text
EASE  = cubic-bezier(0.25, 1.00, 0.50, 1.00)   // movimenti generali, finestre
MORPH = cubic-bezier(0.30, 1.04, 0.42, 1.00)   // morph della barra (lieve overshoot)
```

| Cosa | Durata | Curva |
|---|---|---|
| larghezza, altezza e raggio della barra | 520 ms | MORPH |
| scala/trasformazione della barra | 500 ms | EASE |
| `bottom` della barra | 600 ms | EASE |
| sfondo della barra | 500 ms | lineare |
| finestre: posizione, dimensione, trasformazione, raggio | 450 ms | EASE |
| finestre: opacità | 300 ms | EASE |
| pagine del launchpad | 550 ms | EASE |
| comparsa/scomparsa del layer finestre | 350 ms | EASE |

Il **raggio si anima insieme a larghezza e altezza** con la stessa curva: è questo che fa sembrare la barra un oggetto unico che cambia forma. Gli angoli sono **semplici arrotondamenti circolari** (niente squircle), e il raggio va sempre limitato a `min(r, w/2, h/2)`.

Solver cubic-bezier, da mettere in `shell-anim` se il repo non ne ha uno:

```rust
pub struct CubicBezier { ax: f32, bx: f32, cx: f32, ay: f32, by: f32, cy: f32 }

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        let cx = 3.0 * x1; let bx = 3.0 * (x2 - x1) - cx; let ax = 1.0 - cx - bx;
        let cy = 3.0 * y1; let by = 3.0 * (y2 - y1) - cy; let ay = 1.0 - cy - by;
        Self { ax, bx, cx, ay, by, cy }
    }
    fn x(&self, t: f32) -> f32 { ((self.ax * t + self.bx) * t + self.cx) * t }
    fn y(&self, t: f32) -> f32 { ((self.ay * t + self.by) * t + self.cy) * t }
    fn dx(&self, t: f32) -> f32 { (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx }
    /// progresso temporale 0..1 -> valore 0..1 (può superare 1 con overshoot)
    pub fn eval(&self, x: f32) -> f32 {
        if x <= 0.0 { return 0.0 } if x >= 1.0 { return 1.0 }
        let mut t = x;
        for _ in 0..8 { // Newton
            let e = self.x(t) - x; if e.abs() < 1e-5 { return self.y(t) }
            let d = self.dx(t); if d.abs() < 1e-6 { break } t -= e / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0); t = x; // bisezione di riserva
        for _ in 0..24 { let v = self.x(t); if (v - x).abs() < 1e-5 { break }
            if v < x { lo = t } else { hi = t } t = 0.5 * (lo + hi); }
        self.y(t)
    }
}
pub const EASE: CubicBezier = CubicBezier::new(0.25, 1.0, 0.5, 1.0);
pub const MORPH: CubicBezier = CubicBezier::new(0.3, 1.04, 0.42, 1.0);
```

**Interruzioni**: se arriva un nuovo `go()` a metà di un morph, la nuova animazione parte dal **valore corrente**, non dal target precedente. Niente salti.

### Cross-fade dei contenuti

Ogni stato ha la sua "vista", ancorata in basso al centro della barra e con dimensioni pari alla shape del suo stato:

- **Vista in uscita**: opacità → 0 in 200 ms, scala → 0.97 in 450 ms (EASE), blur 6 px in 250 ms.
- **Vista in entrata**: stesse transizioni al contrario, con **80 ms di ritardo**, così la barra ha già iniziato a cambiare forma.
- Il contenuto è **ritagliato** (clip) sulla shape corrente della barra.
- Solo la vista attiva riceve input.

---

## 4. Contenuto delle viste (riassunto)

Per i dettagli visivi apri la demo con `?s=<stato>` (es. `?s=control`, `?s=music`, `?s=launch`, `?s=notify`, `?s=power`, `?s=win`).

- **Idle (dock)**: logo "t" (apre il launchpad), icone app con un puntino sotto se la finestra è aperta, mini player (copertina + equalizzatore) quando la musica è attiva, e pill di stato con Wi-Fi e **anello batteria**.
- **Anello batteria**: un path che segue il contorno arrotondato della pill, lunghezza normalizzata a 100, tratteggio `batt / 100`. Verde `#34c759` in carica, rosso `#ff453a` se `batt <= 20` e non in carica, altrimenti bianco.
- **Control**: tile Wi-Fi e Bluetooth (con chevron verso le sottopagine), Non disturbare, cursori volume e luminosità, mini player, e il pulsante power che porta a `Power`. Clic sulla tile = toggle, clic sul chevron = sottopagina.
- **CcWifi / CcBt**: switch in alto, lista reti/dispositivi e stato "spento" quando disattivato.
- **Hud**: icona (che cambia con il livello del volume: muto/basso/alto) + barra di livello.
- **Notify**: icona app, nome app, titolo, testo e barra del timer.
- **Music**: copertina, titolo/artista, seek, precedente/play/successivo, shuffle, repeat, like. "Precedente" torna all'inizio se la posizione è oltre 3 s.
- **Lock**: 4 puntini, tastierino 0-9 e ⌫. Funziona anche da tastiera fisica.

---

## 5. Finestre: aspetto

- **Cornice**: raggio 26 px, padding `0 9 9 9`. Gradiente chiaro rosato a 160° (`rgba(242,236,240,.9)` → `rgba(236,220,214,.88)` al 45% → `rgba(220,218,222,.9)`), backdrop blur 30 px con saturazione 1.6, bordo interno 1 px bianco al 55%, e ombra `0 30 80 -24 rgba(0,0,0,.55)` + `0 8 24 -12 rgba(0,0,0,.3)`. Testo `#1c1c20`.
- **Barra del titolo**: alta 60 px. Nome dell'app in alto a sinistra (15 px, peso 500, padding sinistro 17 px).
- **Pill dei controlli** a destra: sfondo bianco al 55%, raggio 22, padding 8/10, gap 9. Tre cerchi da 28 px:
  - riduci a icona: chevron giù, grigio `#4b4b50`
  - schermo intero: frecce diagonali, grigio; diventano frecce "verso dentro" quando la finestra è a schermo intero
  - chiudi: X su arancione `#ff5a1f`
- **Corpo**: due pannelli grigio scuro `#58585c` con gap 8 px.
  - Sidebar: 28% della larghezza (min 150, max 220), raggi asimmetrici `18 6 6 18`.
  - Pannello principale: occupa il resto, raggi `6 18 18 6`.
- **A schermo intero** il raggio della cornice va a 0.

---

## 6. Finestre: comportamento

```rust
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WinMode {
    Normal,
    Maximized { restore: Rect },
    Fullscreen { restore: Rect, was_maximized: bool },
}

pub struct Window {
    pub app: AppId,
    pub rect: Rect,
    pub mode: WinMode,
    pub minimized: bool,
    pub z: u32,
}

pub struct Wm {
    pub wins: Vec<Window>,
    pub z_counter: u32,
    pub fullscreen: Option<AppId>,   // al massimo una finestra a schermo intero
    pub bar_peek: Option<PeekBy>,    // vedi §7
}
```

**Area di lavoro** (dove le finestre "normali" e massimizzate possono stare, cioè sopra la barra):

```rust
fn work_area(screen: Size, k: f32) -> Size { Size { w: screen.w, h: screen.h - (28.0 + 68.0 * k + 18.0) } }
```

### Regole

1. **Apertura**:
   - Dimensione `min(760, W-80) × min(520, H-60)`.
   - Centrata nell'area di lavoro, con cascata `n*28` px (n = numero finestre aperte % 5) e `top >= 20`.
   - Entra con l'animazione **genie al contrario** (vedi sotto).
   - Prende il focus e accende il puntino nel dock.
   - Se l'app ha già una finestra: se è ridotta la ripristina, altrimenti le dà il focus.
2. **Focus / z-order**: `z = ++z_counter`. Se un'altra finestra è a schermo intero, **esce dallo schermo intero** prima di dare il focus.
3. **Drag**: trascina dalla barra del titolo (non dai pulsanti), solo in modalità `Normal`. Durante il drag niente transizioni.
4. **Resize da 8 maniglie** (n, s, e, w, ne, nw, se, sw):
   - Bordi: fascia di 10 px centrata sul bordo, che esclude 18 px agli estremi.
   - Angoli: 20×20 px.
   - Cursori: `ns`, `ew`, `nesw`, `nwse`.
   - **Dimensione minima 460×320**. Sui lati w e n sposta anche x/y, in modo che il bordo opposto resti fermo.
   - Le maniglie sono disattivate in `Maximized` e `Fullscreen`.
5. **Doppio clic sulla parte vuota della barra del titolo** → toggle `Maximized`:
   - Rect massimizzato `{x:14, y:14, w:W-28, h:H-14}` nell'area di lavoro.
   - **La barra resta visibile**.
   - Al secondo doppio clic torna al rect salvato.
   - Non fa nulla se la finestra è a schermo intero.
6. **Pulsante schermo intero (stile macOS)** → toggle `Fullscreen`:
   - Salva il rect, dà il focus alla finestra e la porta a `{0, 0, screen.w, screen.h}` con raggio 0.
   - **Nasconde la barra** e la UI desktop (orologio, pannelli demo).
   - Cambia l'icona e l'etichetta accessibile in "Esci da schermo intero".
   - Di nuovo il pulsante, oppure **Esc** (solo se la barra è `Idle` e non è in peek), riporta la finestra al rect salvato (e allo stato massimizzato se lo era).
7. **Riduci a icona**:
   - Se la finestra è a schermo intero, prima esce dallo schermo intero, poi dopo 460 ms la riduce.
   - Animazione **genie**: traslazione verso il centro dell'icona dell'app nel dock (o verso il centro della barra se l'icona non c'è) e scala `max(0.06, icon_w / win_w)`, con opacità a 0.
   - La finestra non riceve più input.
8. **Chiudi**:
   - Scala 0.94 e fade, poi rimuovi dopo 450 ms e spegni il puntino.
   - Se era a schermo intero: azzera `fullscreen`, mostra la barra, azzera il peek.
9. **Clic sull'icona nel dock**:
   - Finestra non aperta → apri.
   - Ridotta → ripristina.
   - Aperta e in cima → riduci.
   - Altrimenti → focus.
   - Eccezione: l'app Musica avvia il player nella barra.
10. **Cambio risoluzione**: la finestra a schermo intero prende la nuova dimensione dello schermo, quella massimizzata quella della nuova area di lavoro. Ricalcola `K`.
11. **Lock e Launchpad**: il layer delle finestre sfuma a 0 (350 ms) e non riceve input.

Genie, pura geometria:

```rust
fn genie(win: Rect, target: Rect) -> Transform {
    let dx = (target.x + target.w / 2.0) - (win.x + win.w / 2.0);
    let dy = (target.y + target.h / 2.0) - (win.y + win.h / 2.0);
    Transform::translate(dx, dy).then_scale((target.w / win.w).max(0.06))
}
```

Se il compositor supporta la deformazione a mesh, puoi fare un genie "vero" (con la finestra che si restringe a imbuto verso l'icona). Mantieni comunque la stessa durata (450 ms EASE) e lo stesso punto di arrivo.

---

## 7. Barra a schermo intero: auto-hide e peek

Quando `wm.fullscreen.is_some()`:

- La barra in stato `Idle` è **nascosta**: traslata in basso di `altezza + 48 px`, con la stessa transizione di trasformazione di 500 ms EASE.
- I **transitori si vedono comunque**: HUD, notifiche, Silent e Charge compaiono sopra la finestra a schermo intero e poi tornano nascosti. La regola è "nascondi solo se `state == Idle` e non in peek".

```rust
pub enum PeekBy { Mouse, Key }

const EDGE_PX: f32 = 6.0;          // zona di attivazione sul bordo inferiore
const HIDE_DELAY_MS: u32 = 450;

fn on_pointer_move(&mut self, y: f32, screen_h: f32, k: f32, bar: BarState) {
    if self.fullscreen.is_none() { return; }
    let edge = screen_h - y; // distanza dal fondo
    if edge <= EDGE_PX { self.set_peek(Some(PeekBy::Mouse)); return; }
    if self.bar_peek != Some(PeekBy::Mouse) { return; }
    if edge > 28.0 + 68.0 * k + 40.0 {
        // uscito dalla zona della barra: nascondi dopo 450 ms se la barra è ancora Idle
        self.schedule_once(HIDE_DELAY_MS, WmEvent::HidePeekIfIdle);
    } else {
        self.cancel(WmEvent::HidePeekIfIdle);
    }
}
```

- **Mouse**: arrivare entro 6 px dal fondo mostra la barra. Restando nella zona della barra (fino a `28 + 68K + 40` px dal fondo) resta visibile. Uscendo, si nasconde dopo 450 ms, ma **solo se la barra è ancora `Idle`**: se l'utente ha aperto il centro di controllo o il launchpad resta aperta.
- **Tasto Win/Super**: la pressione **da solo** (keydown Super, nessun altro tasto in mezzo, keyup Super) fa toggle della barra (`PeekBy::Key`). Se Super viene usato in combinazione (Super+qualcosa), non fa nulla. In justOS intercetta Super a livello di compositor, prima delle app.
- **Clic dentro una finestra** mentre la barra è aperta col tasto: la richiude.
- **Esc** esce dallo schermo intero solo se la barra è `Idle` e non c'è peek. Altrimenti Esc appartiene alla barra (chiude CC/launchpad).
- Quando si entra o si esce dallo schermo intero il peek si azzera.

---

## 8. Tema

| Token | Valore |
|---|---|
| Brand (accento, logo, selezioni) | `#8c52ff` |
| Vetro della barra | `rgba(18,18,22,.62)` + blur 30 px, saturazione 1.8 |
| Bordo del vetro | 1 px interno bianco a bassa opacità + highlight superiore 1 px `rgba(255,255,255,.07)` |
| Ombra della barra | `0 24 60 -18 rgba(0,0,0,.6)` |
| Sfondo scuro (Spin/Boot/Off) | `#0b0b0f` |
| Verde (carica, PIN ok, switch) | `#34c759` |
| Rosso (batteria bassa, PIN errato) | `#ff453a` |
| Arancione (chiudi finestra) | `#ff5a1f` |

Font:

- **Google Sans Flex** per tutta la UI.
- **Dela Gothic One** solo per il wordmark "ticly" (sempre minuscolo).

Usa i font file che justOS già incorpora. Se mancano, chiedi all'utente prima di aggiungerli: verifica la licenza e non scaricare file da fonti non ufficiali.

---

## 9. Rendering e prestazioni

- La barra è **una surface di sistema** sopra le finestre (z più alto del layer delle finestre e più basso solo di overlay come lo schermo nero di spegnimento).
- Rounded rect con raggio animato: usa un SDF di rettangolo arrotondato nello shader. È anche la base per clip, bordo e ombra, quindi non serve ricostruire la geometria a ogni frame.
- **Backdrop blur**: campiona solo l'area sotto la shape corrente, con un blur a due passate (kawase o gaussiano separabile) a risoluzione ridotta. Fallback senza blur: vetro più opaco (`rgba(18,18,22,.85)`).
- Anima solo durante le transizioni. A riposo niente ridisegni, salvo il player (seek ogni 250 ms) e l'orologio.
- Damage tracking: invalida l'unione tra la shape precedente e quella nuova (più l'ombra).
- Obiettivo: 60 fps stabili (o il refresh del display) durante morph e drag. Misura con gli strumenti di profiling del repo.

---

## 10. Test

Test unitari puri, senza GPU:

- `CubicBezier::eval`: `eval(0)=0` e `eval(1)=1`, curva monotona per EASE, e overshoot di MORPH sopra 1 in qualche punto.
- `BarState::shape`: valori della tabella in §2, e `Idle` a 760 con musica attiva.
- `transient`:
  - rifiutato in `Lock/Off/Boot/Spin/Power/Launch`
  - `Hud` rifiutato in CC
  - torna a `Idle` dopo la durata giusta (usa un clock finto)
- Interruzione del morph: un nuovo `go()` a metà parte dal valore corrente.
- Resize: minimo 460×320 rispettato per tutte le 8 maniglie, con il bordo opposto fermo per `w` e `n`.
- Doppio clic: Normal → Maximized → Normal con il rect originale identico. Ignorato in Fullscreen.
- Fullscreen: enter → rect = schermo, `wm.fullscreen` impostato, barra nascosta. Exit → rect originale (e torna Maximized se lo era).
- Focus su un'altra finestra durante il fullscreen → esce dal fullscreen.
- Peek:
  - mouse entro 6 px → visibile
  - uscita dalla zona → nascosta dopo 450 ms solo se `Idle`
  - Super da solo → toggle
  - Super+tasto → niente
- Chiusura di una finestra fullscreen → `fullscreen = None` e barra visibile.

---

## 11. Checklist di accettazione

Confronta ogni punto affiancando la demo:

- [ ] La barra cambia forma con larghezza, altezza e raggio animati insieme, senza scatti né salti se interrotta.
- [ ] Tutti gli stati della tabella §2 hanno le misure e i tempi giusti, e i transitori tornano a Idle da soli.
- [ ] PIN, centro di controllo (con sottopagine Wi-Fi/BT), HUD, notifiche, Non disturbare, musica, spegnimento/riavvio e ricarica funzionano come nella demo.
- [ ] Il launchpad si apre dal logo, la barra diventa la ricerca, la griglia filtra e si sfoglia a pagine.
- [ ] L'anello batteria segue il bordo della pill, ed è verde in carica e rosso sotto il 20%.
- [ ] Le finestre si aprono dal dock/launchpad con il genie, si spostano, si ridimensionano da bordi e angoli (min 460×320) e si riducono verso l'icona.
- [ ] Il doppio clic sulla barra del titolo massimizza con la barra visibile.
- [ ] Il pulsante schermo intero copre tutto lo schermo e nasconde la barra. Il mouse in basso e Super da solo la mostrano, Esc esce.
- [ ] HUD e notifiche compaiono anche a schermo intero.
- [ ] 60 fps durante morph, drag e resize. Nessun ridisegno a riposo.
- [ ] Nessun dato personale reale nel codice o nei test: usa profili fittizi (es. "Alex Rossi").

---

## 12. Cosa NON fare

- Non copiare l'HTML/CSS/JS della demo dentro justOS: la demo è una specifica, non codice da portare riga per riga.
- Non inventare API di justOS: se manca qualcosa (blur, input globale, timer), proponi l'aggiunta all'utente e mettila nel crate giusto.
- Non mettere il PIN o altre credenziali nel codice: lo sblocco passa dal servizio di autenticazione di sistema.
- Non usare nomi di prodotti o dispositivi reali nella UI.
- Il brand si scrive sempre **ticly** minuscolo.
