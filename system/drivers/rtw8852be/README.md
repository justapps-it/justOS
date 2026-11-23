# rtw8852be: Realtek RTL8852BE (Wi-Fi 6)

Il driver delle schede Wi-Fi Realtek RTL8852BE (PCI `10ec:b852`, `10ec:b85b`).
**Non è finito.** Oggi accende il chip, ne legge la revisione e l'efuse
(indirizzo MAC, tipo di RFE, paese, calibrazione della potenza), sceglie
l'immagine del firmware adatta e ne controlla le sezioni; poi riaccende il
chip con i valori dell'efuse, prepara il DMAC per il caricamento (buffer DLE
e controllo di flusso del canale dei comandi), dà alla scheda gli anelli
DMA, avvia la CPU della scheda in modalità di caricamento e le manda il
firmware (intestazione e sezioni, come pacchetti H2C sul canale 12), e
aspetta che il firmware dica di essere partito. Ci prova cinque volte, come
rtw89. Poi ferma la CPU, spegne il chip e si ferma, senza aggiungere una
radio. Mancano l'inizializzazione di MAC, BB e RF con le tabelle
dell'8852B, il cambio di canale e la trasmissione e ricezione dei dati.
Quando ci saranno, il driver registra la radio con `wifi_register` e il
resto (ricerca delle reti, WPA2/WPA3, DHCP) c'è già: è la stazione del
kernel, provata con `make test-wifi`.

Il codice è diviso così: `fw.rs` (il file del firmware), `desc.rs` (i
descrittori di trasmissione e ricezione e le intestazioni H2C) e `quota.rs`
(le quote dei buffer) formano una libreria provata con `make check`;
`pci.rs` (anelli DMA e impostazione PCIe), `fwdl.rs` (DMAC, CPU della
scheda, caricamento) e `main.rs` (accensione, efuse, i passi) sono il
driver.

Ogni passo scrive una riga `rtw8852be: stage N: ...` nel registro (`log` nel
terminale, o la seriale): sono le righe da mandare dopo un avvio sulla
scheda vera. Se tutto va bene l'ultima è `stage 12: firmware running`,
seguita da `stopping after stage 12`; se un passo non riesce, la riga
`stage N failed` dice cosa aspettava, il valore del registro e lo stato del
caricamento, e le righe dopo (`WCPU_FW_CTRL`, `BOOT_DBG`, il program
counter della CPU) servono a capire perché.

## Licenze

- **Codice.** Registri, bit e sequenze vengono dal driver rtw89 di Linux
  (`rtw8852b.c`, `rtw8852b_common.c`, `rtw8852b_common.h`, `mac.c`,
  `mac.h`, `efuse.c`, `fw.c`, `fw.h`, `pci.c`, `pci.h`, `txrx.h`, `core.c`,
  `reg.h`), Copyright(c) 2019-2022 Realtek Corporation, con licenza "GPL-2.0 OR BSD-3-Clause": justOS li usa con la BSD-3-Clause.
  Il codice è riscritto in Rust, nessuna riga copiata.
- **Firmware.** `firmware/rtw8852b_fw-2.bin` è il file di linux-firmware
  (`rtw89/rtw8852b_fw-2.bin`, sha256 `0b8b054e…87aaba`), non modificato.
  Licenza: `firmware/LICENCE.rtlwifi_firmware.txt` (ridistribuibile solo in
  forma binaria e senza modifiche, con quell'avviso; vietato il reverse
  engineering).
