# DDreams browser

Osobný katalóg a prehliadač fotografií pre Windows, náhrada ACDSee pri dennom triedení.
Tauri 2 + Rust + React/TypeScript + SQLite, s Python modulmi pre AI a dekódovanie RAW.

## Čo vie

- **Prehliadanie:** strom priečinkov aj katalógu, mriežka s virtualizáciou, kalendár, mapa (GPS),
  triedenie podľa názvu, dátumu snímania (EXIF), veľkosti, hodnotenia…; filtre podľa hodnotenia,
  farebného štítka, označenia, typu súboru, priradenia kategórií a kľúčových slov.
- **Prehliadač na celú obrazovku:** okamžitý náhľad, potom plné rozlíšenie (aj dekódovaný RAW cez
  LibRaw, alebo rýchlo vložený náhľad – prepínač „Dekódovať RAW“), zoom na skutočné pixely,
  kontextové menu, rámčeky tvárí s menami.
- **Porovnávací stôl:** 2–4 fotky vedľa seba so synchronizovaným zoomom a posunom (kláves C).
- **Triedenie:** hodnotenie, farebné štítky, označenie, „Na úpravu“, „Na vyradenie“ (karanténa),
  kategórie a hierarchické kľúčové slová, presun/kopírovanie/premenovanie so sidecarmi, história
  zmien s návratom.
- **IPTC:** Popis, Autor a Kľúčové slová sa zobrazujú zo súboru a úprava sa hneď zapíše do súboru
  (JPEG/TIFF/DNG priamo, ostatné RAW do XMP sidecaru `<názov>.xmp` ako ACDSee/Lightroom). Dávkový
  zápis z katalógu iba dopĺňa chýbajúce hodnoty. Zálohy sa nerobia, čas zmeny súboru sa zachová.
- **AI (lokálne, GPU):** vyhľadávanie podľa obsahu textom (aj po slovensky), „Nájsť podobné“,
  návrhy kategórií a kľúčových slov, série a duplicity, technická kvalita (ostrosť, prepal,
  podexpozícia), dominantné farby, detekcia a rozpoznávanie tvárí s návrhmi mien.
- **Import z ACDSee:** katalóg (kategórie, kľúčové slová, hodnotenia, štítky…) aj tváre s menami.

## Požiadavky

- Windows 10/11 x64, WebView2 (súčasť Windows 11)
- [Node.js](https://nodejs.org/) 22+ (vyvíjané na 26), [Rust](https://rustup.rs/) stable (1.96)
  s MSVC toolchainom
- Python 3.14 (python.org, inštalácia do profilu používateľa) – pre AI a RAW
- NVIDIA GPU s CUDA 12 (vyvíjané na RTX 4060 Ti 16 GB). AI potrebuje ~2–4 GB VRAM a ~4 GB RAM.
  Bez GPU aplikácia funguje, AI funkcie budú nedostupné alebo veľmi pomalé.
- [ExifTool](https://exiftool.org/) pre Windows (vyvíjané s 13.29) – rozbaliť tak, aby existoval
  `tools\exiftool\exiftool.exe` a `tools\exiftool\exiftool_files\` (priečinok je v `.gitignore`)
- ~10 GB miesta pre inštaláciu (Python runtime ~4,4 GB, modely ~2 GB, cache)

## Inštalácia od nuly

Inštalácia je prenosná, celý priečinok `C:\DDreams` sa dá presunúť (Python venv však závisí od
základného Pythonu 3.14 na danom počítači).

```powershell
npm install
npx tauri build --no-bundle                     # ~7 min, vytvorí src-tauri\target\release\ddreams-browser.exe
powershell -ExecutionPolicy Bypass -File tools\setup-python.ps1    # venv C:\DDreams\modules\python z ai\requirements.txt
C:\DDreams\modules\python\Scripts\python.exe -m pip install --target C:\DDreams\modules\ai\pylibs -r ai\requirements-faces.txt
C:\DDreams\modules\python\Scripts\python.exe tools\prepare_models.py --modules C:\DDreams\modules
powershell -ExecutionPolicy Bypass -File tools\publish.ps1
```

`publish.ps1` skopíruje aplikáciu, AI server, RAW plugin a ExifTool do `C:\DDreams`
(`-InstallRoot` mení cieľ). Pri prvej inštalácii vytvorí prázdny katalóg (`tools\create_catalog.py`:
bez priečinkov a kľúčových slov, len vzorové kategórie Ľudia › Rodina a Miesta). S `-Catalog <cesta>`
namiesto toho skopíruje existujúci katalóg, napríklad z importu ACDSee. Existujúci katalóg nikdy neprepíše.

Fotky sa do katalógu pridávajú priebežne: stačí otvoriť priečinok a fotke priradiť hodnotenie,
kategóriu alebo kľúčové slovo. Celý priečinok zaradíš ako „zahrnutý“ v Nastaveniach alebo pravým klikom
v strome priečinkov: jeho nové fotky sa potom pridajú do katalógu pri každom otvorení.
Ďalšie nasadenia: zavrieť `C:\DDreams\ddreams-browser.exe`, postaviť a znova spustiť `publish.ps1`.

Štruktúra inštalácie:

```
C:\DDreams\
  ddreams-browser.exe, settings.json
  catalog\   catalog.sqlite, ai.sqlite (index AI), faces.sqlite
  cache\     náhľady a plné rozlíšenie
  backups\   zálohy katalógu
  modules\   python\ (venv), exiftool\, raw\ddreams_raw.py,
             ai\ddreams_ai.py, ai.json, model\, faces\, pylibs\
```

### AI modely (nie sú v gite)

`tools/prepare_models.py` ich stiahne, overí kontrolné súčty a pripraví; existujúce súbory nechá.

| Model | Zdroj | Cieľ | Licencia |
|---|---|---|---|
| SigLIP2 ViT-L-16-256 (obsah, podobnosť, návrhy) | Hugging Face `timm/ViT-L-16-SigLIP2-256` @ `da94269`, prevedený na fp16 (~1,76 GB) | `modules\ai\model\` | Apache-2.0 |
| InsightFace buffalo_l: `det_10g.onnx`, `w600k_r50.onnx` (tváre) | [GitHub release v0.7](https://github.com/deepinsight/insightface/releases/tag/v0.7) | `modules\ai\faces\` | len nekomerčné použitie |

`onnxruntime-gpu` pre tváre musí ostať na 1.24.x (novšie vyžadujú CUDA 13, torch je cu126).

### Katalóg z ACDSee

Importer používa iba štandardnú knižnicu Pythonu a číta **kópiu** databázy ACDSee:

```powershell
python tools\import_acdsee\import_acdsee.py --source <kópia databázy ACDSee> --out .local\catalog.sqlite
python ai\import_acdsee_faces.py --help         # import tvárí a mien z tej istej kópie
```

Takto vytvorený katalóg sa pri prvej inštalácii odovzdá cez `publish.ps1 -Catalog <cesta>`.

## Vývoj

```powershell
npm run tauri dev
```

Vývojová verzia používa katalóg `.local\catalog.sqlite` v repozitári (aj `ai.sqlite`, `faces.sqlite`),
cache `G:\ddreams-work\thumb-cache`, ExifTool z `tools\exiftool` a Python/model z `C:\DDreams\modules`.
Inštalácia sa rozpozná podľa `catalog\catalog.sqlite` vedľa exe (alebo súboru `ddreams.portable`).

Premenné prostredia: `DDREAMS_CATALOG`, `DDREAMS_CACHE`, `DDREAMS_EXIFTOOL`, `DDREAMS_FORMATS`,
`DDREAMS_METADATA_VIEW`, `DDREAMS_READONLY=1` (katalóg iba na čítanie).

Testy:

```powershell
cd src-tauri; cargo test; cd ..
npx tsc --noEmit
node --test tools/*.test.mjs
cd ai;  C:\DDreams\modules\python\Scripts\python.exe -B -m unittest; cd ..
cd raw; C:\DDreams\modules\python\Scripts\python.exe -B -m unittest; cd ..
```

### Rozšírenia

- **Formáty:** `src-tauri/formats.json` (`{ "ext": "cr3", "group": "raw", "name": "Canon RAW CR3" }`).
  Súbor v `modules\formats.json` nahradí vstavaný zoznam.
- **Zobrazenie metadát:** `src-tauri/metadata_view.json`, prepísateľné cez `modules\metadata_view.json`.
- **Zdroje náhľadov:** `PreviewSource` v `src-tauri/src/thumbs.rs` (vstavaný JPEG → ExifTool → LibRaw →
  Windows Shell).

### Pravidlá, ktoré platia v celom kóde

- Fotky sa nikdy nemažú natrvalo bez potvrdenia; vyradené idú do karantény alebo Koša.
- Do RAW súborov (okrem DNG) sa nikdy nezapisuje, metadáta idú do XMP sidecaru.
- `import rawpy` v `raw/ddreams_raw.py` musí ostať na úrovni modulu (lenivý import v pracovnom vlákne
  na Windows zamrzne).
