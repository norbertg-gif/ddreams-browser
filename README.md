# DDreams browser

Read-only katalóg fotografií pre Windows, s rozhraním inšpirovaným režimom Manage v ACDSee.

## Spustenie

V koreňovom priečinku projektu spustite `npm run tauri dev`. Katalóg `.local/catalog.sqlite` musí byť vytvorený vopred; cestu možno zmeniť cez `DDREAMS_CATALOG`.

## Import katalógu

Importer používa iba štandardnú knižnicu Pythonu:

```powershell
python tools\import_acdsee\import_acdsee.py --source G:\ddreams-work\acdsee-copy --out .local\catalog.sqlite
```

## Formáty a zdroje náhľadov

Zoznam prípon a ich skupín je v `src-tauri/formats.json`. Pridanie podporovanej prípony znamená pridať jej malú položku `{ "ext": "cr3", "group": "raw", "name": "Canon RAW CR3" }`. Súbor vedľa spustiteľného programu nahradí vstavaný zoznam; premenná `DDREAMS_FORMATS` určuje inú cestu. Neplatný súbor sa zaloguje a aplikácia použije vstavaný zoznam.

Zdroj náhľadu pridajte implementáciou `PreviewSource` v `src-tauri/src/thumbs.rs` a zaraďte ho do `PreviewChain::new` v požadovanom poradí/skupinách. Reťazec pokračuje pri neúspešnom zdroji. RAW súbory zatiaľ používajú vložené náhľady z ExifTool; RAW dekódovanie nie je implementované.

## Režim iba na čítanie

Priečinkové zobrazenie číta obsah disku a katalóg používa ako prekrytie metadát. SQLite sa otvára cez `mode=ro`. Zápis je povolený iba do cache náhľadov (predvolene `G:\ddreams-work\thumb-cache`, nastaviteľné cez `DDREAMS_CACHE`) a do lokálneho nastavenia rozhrania. Niektoré Huawei/Honor DNG, napríklad `IMG_20190517_114021.dng`, nemajú vložený náhľad a zobrazia typovú dlaždicu.
