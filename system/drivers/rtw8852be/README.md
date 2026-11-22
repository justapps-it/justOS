# rtw8852be: Realtek RTL8852BE (Wi-Fi 6)

Il driver delle schede Wi-Fi Realtek RTL8852BE (PCI `10ec:b852`, `10ec:b85b`).
**Non è finito.** Oggi accende il chip, ne legge la revisione e l'efuse
(indirizzo MAC, tipo di RFE, paese), sceglie l'immagine del firmware adatta
e ne controlla le sezioni; poi spegne il chip e si ferma, senza aggiungere
una radio. Mancano il caricamento del firmware (anelli DMA, pacchetti H2C,
avvio della CPU della scheda), l'inizializzazione di MAC, BB e RF con le
tabelle dell'8852B, il cambio di canale e i descrittori di trasmissione e
ricezione. Quando ci saranno, il driver registra la radio con
`wifi_register` e il resto (ricerca delle reti, WPA2/WPA3, DHCP) c'è già:
è la stazione del kernel, provata con `make test-wifi`.

Ogni passo scrive una riga `rtw8852be: stage N: ...` nel registro (`log` nel
terminale, o la seriale): sono le righe da mandare dopo un avvio sulla
scheda vera.

## Licenze

- **Codice.** Registri, bit e sequenze vengono dal driver rtw89 di Linux
  (`rtw8852b.c`, `rtw8852b_common.h`, `mac.c`, `mac.h`, `efuse.c`, `fw.c`,
  `fw.h`, `core.c`, `reg.h`), Copyright(c) 2019-2022 Realtek Corporation,
  con licenza "GPL-2.0 OR BSD-3-Clause": justOS li usa con la BSD-3-Clause.
  Il codice è riscritto in Rust, nessuna riga copiata.
- **Firmware.** `firmware/rtw8852b_fw-2.bin` è il file di linux-firmware
  (`rtw89/rtw8852b_fw-2.bin`, sha256 `0b8b054e…87aaba`), non modificato.
  Licenza: `firmware/LICENCE.rtlwifi_firmware.txt` (ridistribuibile solo in
  forma binaria e senza modifiche, con quell'avviso; vietato il reverse
  engineering).
