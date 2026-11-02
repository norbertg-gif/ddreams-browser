# DDreams browser

Read-only katalóg fotografií pre Windows. Rozhranie je inšpirované správou katalógu v ACDSee.

## Spustenie

V koreňovom priečinku projektu spustite `npm run tauri dev`. Predtým musí existovať `.local/catalog.sqlite`. Cestu možno zmeniť premennou `DDREAMS_CATALOG`.

## Import katalógu

Importer používa iba štandardnú knižnicu Pythonu:

```powershell
G:\Programovanie\photosort\.venv\Scripts\python.exe tools\import_acdsee\import_acdsee.py --source G:\ddreams-work\acdsee-copy --out .local\catalog.sqlite
```

Po zmene schémy alebo indexov import zopakujte nad kópiou databázy ACDSee.

## Funkcie alpha

Zobrazenie priečinka číta aktuálny obsah disku a databázu používa ako prekrytie metadát. Kategórie, keywordy, hodnotenia a označenia sú založené na katalógu. Súbory sa otvárajú iba na čítanie; SQLite sa otvára v režime `mode=ro`. Zápis je povolený len do cache náhľadov (predvolene `G:\ddreams-work\thumb-cache`, nastaviteľné cez `DDREAMS_CACHE`) a do lokálneho nastavenia rozhrania.

Náhľady RAW používajú najväčší vložený náhľad. Niektoré Huawei/Honor DNG, napríklad `IMG_20190517_114021.dng`, vložený náhľad nemajú a zobrazia typovú dlaždicu.

## Klávesové skratky

- `Alt+←` / `Alt+→`: späť / vpred; `Backspace`: nadradený priečinok; `F5`: obnoviť
- Šípky: pohyb výberu; `Shift+šípky`: rozšíriť výber; `Ctrl+klik`: prepínať výber
- `Ctrl+A`: všetky položky; `Ctrl+Shift+A`: všetky súbory; `Ctrl+I`: fotografie
- `Ctrl+Shift+I`: invertovať výber; `Ctrl+Shift+D`: zrušiť výber
- `]`: prepínať prekrytia; `Ctrl+koliesko`: meniť veľkosť dlaždíc
- `Ctrl+Shift+O`: otvoriť vybraný súbor v predvolenom programe; `Ctrl+G`: kopírovať zoznam výsledkov
- V paneli metadát: `Ctrl+A` vyberie riadky, `Ctrl+C` ich skopíruje

Úpravy metadát, hodnotenie, tagovanie, presúvanie a mazanie nie sú súčasťou tejto alpha verzie.
