# DDreams browser

Read-only katalóg fotografií pre Windows (Tauri 2 + React). Zobrazenie je inšpirované správou katalógu v ACDSee.

## Spustenie

V koreňovom priečinku projektu spustite `npm run tauri dev`. Predtým musí existovať `.local/catalog.sqlite`; predvolenú cestu možno zmeniť premennou `DDREAMS_CATALOG`.

## Import katalógu

Importer používa iba Python štandardnú knižnicu:

```powershell
G:\Programovanie\photosort\.venv\Scripts\python.exe tools\import_acdsee\import_acdsee.py --source G:\ddreams-work\acdsee-copy --out .local\catalog.sqlite
```

## Režim alpha

Databáza sa otvára cez SQLite `mode=ro`; fotografia a metadáta sa iba čítajú. Zápis je povolený len do cache náhľadov (predvolene `G:\ddreams-work\thumb-cache`, zmeniť cez `DDREAMS_CACHE`). Offline súbory zobrazia náhradný stav. Úpravy, presúvanie ani mazanie nie sú súčasťou alpha.

## Known limitation

Huawei/Honor DNG files without an embedded preview (for example IMG_20190517_114021.dng) currently show the file-type placeholder.
