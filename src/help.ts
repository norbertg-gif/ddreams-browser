export type HelpBlock =
  | { type: "paragraph"; text: string }
  | { type: "note"; text: string }
  | { type: "bullets"; items: string[] }
  | { type: "steps"; items: string[] }
  | { type: "shortcuts"; rows: [string, string][] };
export type HelpTopic = { id: string; title: string; keywords: string[]; blocks: HelpBlock[] };

export const helpTopics: HelpTopic[] = [
  { id: "quarantine", title: "Import a karanténa", keywords: ["Panasonic", "import", "vymazanie", "kôš"], blocks: [
    { type: "paragraph", text: "V Nastaveniach → Import a karanténa vyberte zdroj, karanténu, lehoty a najstarší dátum snímania. Lehota presunu sa počíta od prvého záznamu v importe. Hodnota 0 danú kontrolu vypne." },
    { type: "paragraph", text: "Pri štarte môžete skontrolovať fotky bez hodnotenia, štítka, kategórie a označenia, ktoré už majú lehotu presunu za sebou. Kontrola zahŕňa aj staršie položky karantény. Presun a odstránenie sa vykonajú až po potvrdení." },
    { type: "paragraph", text: "Do karantény môžete potvrdením presunúť aj vybrané fotky alebo návrhy na vymazanie. V položke Katalóg → Karanténa fotky vrátite na pôvodné miesto alebo ich odstránite do Koša. Disk bez Koša vyžaduje potvrdenie trvalého zmazania." }
  ] },
  { id: "start", title: "Prvé kroky", keywords: ["začiatok", "prehľad"], blocks: [
    { type: "steps", items: ["V paneli Priečinky rozbaľte disk a vyberte priečinok; v strede sa zobrazia jeho položky.", "Kliknutím vyberte fotografiu, dvojklikom ju otvorte. Údaje o výbere sú v paneli Vlastnosti.", "V paneli Katalóg vyberte kategóriu alebo inú skupinu; pomocou Filtre a poľa Filtrovať v zobrazení… zúžte výsledky."] },
    { type: "note", text: "Pomocníka otvoríte klávesom F1 alebo z ponuky Zobrazenie. Témy možno vyhľadávať aj bez diakritiky." }
  ] },
  { id: "workspace", title: "Panely a rozloženie", keywords: ["okno", "vlastnosti", "katalóg"], blocks: [
    { type: "paragraph", text: "Priečinky zobrazujú strom diskov a skratky, Kalendár dátumy, Vlastnosti údaje vybranej fotografie, Katalóg triedenie a AI nástroje na prehľadávanie a kontrolu fotografií." },
    { type: "bullets", items: ["Kliknutím na záložku aktivujte panel; šípkou na jeho okraji ho zbaľte alebo rozbaľte.", "Záložku panela možno potiahnuť do iného stĺpca alebo medzi záložky. Pravý klik na záložku ponúka aj Obnoviť pôvodné rozloženie.", "Potiahnutím deliacej čiary zmeňte šírku panelu. V Nastaveniach možno zmeniť veľkosť písma rozhrania."] }
  ] },
  { id: "dock-layout", title: "Rozloženie panelov", keywords: ["sekcie", "stĺpce", "záložky", "presun"], blocks: [
    { type: "bullets", items: [
      "Sekciu presuňte za úchyt v hlavičke. Pustite ju nad hornou alebo dolnou polovicou inej sekcie, na lište záložiek alebo pri deliacej čiare či okraji okna, ak chcete vytvoriť nový stĺpec.",
      "Pravým klikom na hlavičku sekciu skryjete. Znova ju zobrazíte z ponuky Rozloženie.",
      "V ponuke Rozloženie si uložte pomenované rozloženia a prepínajte medzi nimi. Ukladá sa aj šírka stĺpcov, viditeľnosť a rozbalenie sekcií. Tu môžete obnoviť aj pôvodné rozloženie."
    ] }
  ] },
  { id: "folders", title: "Priečinky a katalóg", keywords: ["disk", "zdroj", "skratky", "offline", "vylúčenie"], blocks: [
    { type: "paragraph", text: "Strom Priečinky ukazuje existujúce priečinky na disku. Šípkou rozbaľte podpriečinky, kliknutím otvorte priečinok. Horné šípky slúžia na návrat, postup dopredu, prechod vyššie a obnovenie." },
    { type: "bullets", items: ["Aktuálny priečinok pridáte do časti Skratky tlačidlom +. Pravým klikom na skratku možno zmeniť jej názov alebo poradie či ju odstrániť zo zoznamu; samotný priečinok zostane.", "Pravým klikom na priečinok alebo v Nastaveniach možno nastaviť zahrnutie či vylúčenie z katalógu. Nové fotky a videá zo zahrnutého priečinka sa pridajú pri jeho otvorení. Vylúčený priečinok sa dá ďalej prehliadať. Sýta zelená bodka označuje položky priamo v priečinku alebo vlastné pravidlo zahrnutia, svetlá označuje položky v podpriečinkoch a polovičná upozorňuje na vylúčené podpriečinky.", "V Katalógu sú Špeciálne položky: Všetky fotky, Bez kategórie, Bez keywordov a Označené. OFFLINE označuje odpojený disk, CHÝBA neexistujúci súbor na pripojenom disku.", "V Nastaveniach v časti Offline a chýbajúce zdroje možno taký zdroj odstrániť z katalógu. Odstránia sa aj jeho údaje v katalógu; súbory sa nemažú. Pred odstránením vznikne záloha katalógu."] }
  ] },
  { id: "selection", title: "Náhľady a výber", keywords: ["mriežka", "veľkosť", "označenie"], blocks: [
    { type: "bullets", items: ["Veľkosť náhľadov zmeňte posúvačom v hornej lište. Voľba Zobrazenie nastavuje náhľad pri prejdení myšou a ikonky na náhľadoch.", "V Zobrazenie → Údaje pod náhľadmi… vyberte zobrazené polia, ich poradie a počet riadkov.", "Klik vyberie položku, Ctrl+klik mení jednotlivý výber a Shift+klik vyberie rozsah. Ťahaním myši na prázdnej ploche možno vyznačiť obdĺžnik; Ctrl pridáva k výberu.", "Pravý klik na náhľad otvorí kontextovú ponuku. Vybrané súbory možno potiahnuť na priečinok v strome; s Ctrl sa kopírujú, inak presúvajú."] }
  ] },
  { id: "viewer", title: "Prehliadač a RAW", keywords: ["fotografia", "prezentácia", "priblíženie", "filmstrip"], blocks: [
    { type: "paragraph", text: "Fotografiu otvoríte dvojklikom na náhľad alebo Enterom. Dolný pás náhľadov umožňuje prejsť na inú fotografiu; na posun slúžia aj šípky." },
    { type: "bullets", items: ["Kolieskom myši alebo klávesmi + a − zmeňte priblíženie; zväčšený obrázok možno posúvať ťahaním. Tlačidlo 100 % / Prispôsobiť prepína veľkosť a zámok zachová priblíženie pri prechode.", "Spustiť prezentáciu prehráva fotografie v aktuálnom výsledku; interval nastavíte v hornej lište prehliadača.", "Voľba Dekódovať RAW v lište zapína plné rozlíšenie; po vypnutí sa rýchlejšie zobrazí najväčší vložený náhľad, ak je dostupný.", "Lišta uvádza zdroj obrázka a dostupné rozmery originálu. Pri zobrazení originálu alebo plného dekódovania zodpovedá 100 % jednému pixelu obrázka na jeden pixel CSS. Prehliadač opustíte tlačidlom Zavrieť alebo Esc."] }
  ] },
  { id: "filters", title: "Hľadanie, filtre a radenie", keywords: ["vyhľadávanie", "triedenie", "typ", "hodnotenie"], blocks: [
    { type: "paragraph", text: "Pole Filtrovať v zobrazení… zužuje aktuálne zobrazenie podľa názvu a textových údajov. Výber typu nad mriežkou ponúka Všetko, Fotografie, RAW a Video." },
    { type: "bullets", items: ["Ponuka Filtre umožňuje obmedziť výsledky podľa hodnotenia, farebného štítku, označenia a typu súboru. Tlačidlo Vymazať filtre ich odstráni.", "V rozbaľovacom zozname nad mriežkou zvoľte napríklad Názov, Dátum nasnímania, Veľkosť alebo Hodnotenie a vedľajšou šípkou zmeňte smer radenia.", "V Katalógu kliknutím na hodnotenie, farebný štítok, kategóriu alebo kľúčové slovo zobrazíte zodpovedajúce fotografie. Pole Filtrovať kategórie a keywordy… hľadá v strome kategórií a slov."] }
  ] },
  { id: "categories", title: "Kategórie a kľúčové slová", keywords: ["priradenie", "strom", "podkategórie", "keyword"], blocks: [
    { type: "steps", items: ["Vyberte jednu alebo viac fotografií a v paneli Katalóg zapnite Štvorčeky.", "Kliknutím na štvorček pri kategórii alebo kľúčovom slove priraďte alebo odoberte položku výberu. Neúplné priradenie označuje pomlčka.", "Tlačidlom + alebo pravým klikom vytvorte kategóriu či kľúčové slovo. Kontextová ponuka ponúka aj premenovanie, presun a zmazanie."] },
    { type: "paragraph", text: "Ikonu vyberiete pri vytvorení kategórie alebo cez Zmeniť ikonu… a voľbou Zdediť od nadradenej prevezmete ikonu najbližšej nadradenej kategórie s vlastnou ikonou." },
    { type: "paragraph", text: "V strome možno položky presúvať ťahaním. Voľba Vrátane podkategórií rozšíri zobrazenie aj o potomkov. Položku s priradenými súbormi alebo priradeniami v jej podstromoch nemožno zmazať, kým sa priradenia neodoberú." },
    { type: "paragraph", text: "V časti Upraviť vybrané fotky možno tiež odstrániť kategóriu, pridať alebo odobrať kľúčové slovo a zmeniť hodnotenie, štítok či označenie." }
  ] },
  { id: "auto", title: "Auto kategórie a kalendár", keywords: ["EXIF", "fotoaparát", "objektív", "ISO", "čas", "dátum"], blocks: [
    { type: "paragraph", text: "V časti Auto kategórie rozbaľte skupinu a kliknite na hodnotu; fotografie možno vyberať podľa údajov ako fotoaparát, objektív, ISO, clona, čas expozície alebo ohnisková vzdialenosť, vrátane dostupných rozsahov. Počas načítania sa zobrazuje priebeh indexovania EXIF." },
    { type: "paragraph", text: "V paneli Kalendár vyberte Udalosti, Rok, Mesiac alebo Deň a zdroj dátumu. Kliknutím na rok, mesiac alebo deň s fotografiami zobrazíte príslušný výber. Bez dátumu zobrazí položky bez platného dátumu pre zvolený zdroj." }
  ] },
  { id: "metadata", title: "Vlastnosti a metadáta", keywords: ["EXIF", "kopírovanie", "riadky", "schránka"], blocks: [
    { type: "paragraph", text: "Panel Vlastnosti zobrazuje údaje vybranej fotografie po skupinách. Skupiny sa rozbaľujú kliknutím. Riadky možno ťahaním presúvať v rámci rovnakej skupiny a podskupiny." },
    { type: "paragraph", text: "Kliknutím zaškrtnite riadky a pod údajmi vyberte S popisom alebo Bez popisu; šípka ponúka kopírovanie celej sekcie a obnovenie poradia riadkov. Pri viacerých vybraných súboroch sa údaje kopírujú pre jednotlivé súbory." },
    { type: "note", text: "Vlastnosti slúžia na prehliadanie; hodnotenie, štítky, kategórie a ďalšie priradenia sa upravujú v paneli Katalóg." }
  ] },
  { id: "files", title: "Súbory, Kôš a história", keywords: ["kopírovanie", "presun", "mazanie", "história", "bezpečnosť"], blocks: [
    { type: "paragraph", text: "Pravým klikom na výber otvorte ponuku operácií so súbormi. Kopírovanie a presun vyžadujú cieľový priečinok. Pri konflikte názvov možno súbory preskočiť alebo ponechať oba s pridaným číslom; existujúci cieľový súbor sa neprepisuje." },
    { type: "bullets", items: ["Premenovanie mení názov súboru; pri súboroch so sprievodným .xmp sa pracuje aj s ním. V strome priečinkov sú dostupné aj operácie s priečinkami.", "Delete vyžiada potvrdenie presunu do Koša. Shift+Delete zvolí trvalé zmazanie; ak disk nemá Kôš, potvrdenie upozorní na trvalé zmazanie aj pri bežnom Delete. Sprievodné .xmp sa mažú spolu so súbormi.", "Kliknutím na Katalóg v hornej lište otvorte Históriu zmien s časom a súhrnom operácií. Súbory presunuté do Koša možno obnoviť v Prieskumníkovi Windows.", "V režime Katalóg iba na čítanie sú zápisové operácie obmedzené. Vylúčenie priečinka z katalógu nemaže súbory na disku, ale odstráni jeho údaje z katalógu; upozornenie uvádza rozsah."] },
    { type: "note", text: "Aplikácia umožňuje zmeny a mazanie súborov po zvolení príslušnej operácie. Súbory preto možno zmeniť alebo trvalo odstrániť." }
  ] },
  { id: "editors", title: "Externé editory", keywords: ["editor", "program", "otvoriť"], blocks: [
    { type: "paragraph", text: "V Nastaveniach v časti Externé editory vyberte Pridať program…, nastavte názov a prípadne viac súborov. Prvý program je predvolený; poradie možno meniť šípkami alebo tlačidlom Predvolený." },
    { type: "paragraph", text: "Vybrané fotografie otvorte v editore z kontextovej ponuky alebo v prehliadači tlačidlom Otvoriť v:. Ctrl+Alt+X používa predvolený editor." }
  ] },
  { id: "ai", title: "AI vyhľadávanie a návrhy", keywords: ["podobnosť", "duplicity", "index", "skupiny", "farby", "kvalita"], blocks: [
    { type: "paragraph", text: "Po výbere fotiek z priečinka nastaveného ako Priečinok s novými fotkami ponúkne panel AI v časti Kam to patrí? až tri ciele presunu podľa podobných fotiek." },
    { type: "paragraph", text: "Červené ✕ označuje slabší záber zo série, ktorý je možné zobraziť cez Návrh na vymazanie a označenie zrušiť voľbou Ponechať." },
    { type: "paragraph", text: "AI zapnite v Nastaveniach alebo cez stav AI v hornej lište. Stav zobrazuje indexované položky, front, offline a chyby; indexovanie možno pozastaviť, obnoviť alebo aktualizovať. Funkcie závisia od dostupnosti a pripravenosti AI." },
    { type: "paragraph", text: "Voľba Načítať AI model pri štarte je predvolene zapnutá a zmena sa uplatní pri ďalšom zapnutí AI. Po vypnutí voľby sa model načíta pri hľadaní obsahom alebo po stlačení Spracovať čakajúce fotografie; tieto ovládacie prvky sú označené vyžaduje AI model. Už uložené údaje možno používať aj bez modelu; názov skupiny sa zobrazí, ak je jeho textový vektor uložený." },
    { type: "bullets", items: ["Do poľa Hľadať obsahom (AI)… zadajte opis a stlačte Enter. Výsledky sú zoradené podľa zhody; pri neúplnom indexe sa zobrazí počet prehľadaných položiek.", "Panel AI obsahuje DUPLICITY (AI): pri vybranej fotografii alebo v celom katalógu hľadajte Rovnaké alebo Veľmi podobné. NÁVRHY AI zoskupujú návrhy kategórií a kľúčových slov na kontrolu.", "V časti Upraviť vybrané fotky možno kliknutím návrh AI prijať alebo krížikom odmietnuť. V paneli AI možno nájsť NEZARADENÉ SKUPINY, zobraziť ich, priradiť kategóriu alebo ich ignorovať.", "V Katalógu časť FARBY filtruje podľa obsahu fotografie; Ctrl+klik kombinuje viac farieb. V časti úprav možno farby ručne upraviť, Alt+klik obnoví automatickú hodnotu. TECHNICKÁ KVALITA v paneli AI ponúka Neostré, Prepálené a Tmavé.", "Ponuka Série vyhľadá podobné zábery v aktuálnom zobrazení s nastavením Prísne, Stredné alebo Voľné. Označenie na vyradenie používa sivý štítok; samo osebe fotografiu nemaže."] }
  ] },
  { id: "faces", title: "Osoby a tváre", keywords: ["rozpoznávanie", "mená", "kontrola"], blocks: [
    { type: "paragraph", text: "V Katalógu v časti Osoby možno vytvoriť osobu, otvoriť jej fotografie, prepočítať rozpoznanie a v ponuke osoby skontrolovať nepotvrdené návrhy. Model tvárí musí byť dostupný." },
    { type: "paragraph", text: "V časti Upraviť vybrané fotky možno osobu pridať alebo odobrať. V paneli Vlastnosti pri zistených tvárach možno zmeniť osobu, návrh potvrdiť či odmietnuť alebo zvoliť Nie je osoba (ignorovať). Časť Neznáme tváre ponúka hľadanie skupín." }
  ] },
  { id: "map", title: "Mapa katalógu", keywords: ["vizuálna mapa", "ostrovy", "podobnosť"], blocks: [
    { type: "paragraph", text: "Nad stredom zvoľte Mapa a podľa potreby Vypočítať mapu. Body predstavujú fotografie a ostrovy ich vizuálne skupiny; nejde o geografickú mapu." },
    { type: "paragraph", text: "Ťahaním posúvajte mapu a kolieskom ju približujte. Kliknutím vyberte bod, dvojklikom fotografiu otvorte. Celá mapa obnoví pohľad, Prepočítať vytvorí nový výpočet a Viditeľné v mriežke otvorí viditeľné fotografie v mriežke." }
  ] },
  { id: "backup", title: "Zálohy a nastavenia", keywords: ["záloha", "pripomienka", "zobrazenie"], blocks: [
    { type: "paragraph", text: "Ozubeným kolieskom otvorte Nastavenia. V časti Záloha katalógu použite Zálohovať teraz a nastavte Pripomínať zálohu. Pripomienka ponúka zálohovanie, odklad o týždeň alebo Teraz nie." },
    { type: "note", text: "Záloha sa týka katalógu; na uchovanie originálnych fotografií je potrebná samostatná záloha súborov." },
    { type: "paragraph", text: "Nastavenia obsahujú aj pravidlá priečinkov, offline zdroje, zobrazenie iných súborov, veľkosť písma, externé editory a zapnutie AI." }
  ] },
  { id: "pending-edit", title: "Na úpravu", keywords: ["U", "Hotovo", "karanténa", "čaká"], blocks: [
    { type: "paragraph", text: "Vybrané fotky označíte klávesom U, v ponuke náhľadu alebo v paneli úprav. Pri zmiešanom výbere U označí aj zvyšné fotky, pri označenom výbere stav zruší. V špeciálnej položke Na úpravu sú najstaršie označenia navrchu." },
    { type: "paragraph", text: "Stav zrušíte tlačidlom Hotovo. Katalóg ho zruší aj pri zistení zmeny súboru viac ako 60 sekúnd po označení alebo pri zaevidovaní novšieho súboru s rovnakým názvom a inou príponou v tom istom priečinku (napríklad PSD alebo TIFF)." },
    { type: "paragraph", text: "Po 90 dňoch sa pri štarte ponúkne presun dlho čakajúcich fotiek z importu do karantény. Skupinu môžete v kontrole odškrtnúť. Lehotu zmeníte v časti Import a karanténa a hodnotou 0 ponuku vypnete." }
  ] },
  { id: "shortcuts", title: "Klávesové skratky", keywords: ["klávesnica", "klávesy", "F1", "Ctrl", "Shift"], blocks: [
    { type: "note", text: "Skratky závisia od aktívnej oblasti. Pri písaní do polí sa globálne skratky pre prácu s fotografiami nepoužívajú." },
    { type: "shortcuts", rows: [
      ["F1 / Esc", "Otvoriť / zavrieť Pomocníka; Esc zatvára aj ďalšie dialógy a prehliadač."],
      ["Ctrl+1 až 5 / Ctrl+0", "Hodnotenie 1 až 5 / odstrániť hodnotenie."],
      ["Alt+1 až 6 / Alt+0", "Červená, žltá, zelená, modrá, fialová, Na vyradenie / odstrániť štítok."],
      ["U", "Prepnúť stav Na úpravu pre vybrané fotky."],
      ["X / spätná lomka", "Prepnúť štítok Na vyradenie / označenie vybraných."], ["Ctrl+Alt+X", "Otvoriť výber v predvolenom editore."],
      ["Ctrl+C / Ctrl+X / Ctrl+V", "V mriežke alebo v strome priečinkov kopírovať / vystrihnúť / vložiť."],
      ["F2 / Delete / Shift+Delete", "V mriežke premenovať / zmazať / zmazať natrvalo; F2 a Delete fungujú aj v strome priečinkov. F2 premenuje aj aktívnu kategóriu alebo kľúčové slovo."],
      ["Ctrl+spätná lomka / Ctrl+G", "V mriežke vybrať označené / kopírovať aktuálny výsledok."], ["Ctrl+Shift+O", "V mriežke otvoriť vybraný súbor."],
      ["Alt+← / Alt+→ / Backspace / F5", "V mriežke alebo prehliadači: späť / dopredu / vyššie / obnoviť."], ["]", "V mriežke alebo prehliadači prepnúť ikonky na náhľadoch."],
      ["Ctrl+A / Ctrl+Shift+A", "V mriežke vybrať všetky položky / všetky súbory."], ["Ctrl+I / Ctrl+Shift+I / Ctrl+Shift+D", "V mriežke vybrať fotografie / obrátiť výber / zrušiť výber."],
      ["Šípky, Home, End, Page Up, Page Down", "V mriežke presunúť výber; Ctrl presúva zameranie, Shift rozširuje rozsah."], ["Ctrl+medzerník / Enter", "V mriežke prepnúť výber zameranej položky / otvoriť vybranú položku."],
      ["Enter / Esc", "V prehliadači zavrieť fotografiu; Enter na vybranom bode mapy otvorí fotografiu."],
      ["← / → / medzerník / Backspace", "V prehliadači predchádzajúca / nasledujúca fotografia, aj pri priblížení."], ["+ alebo = / −", "V prehliadači priblížiť / oddialiť."], ["↑ / ↓ / Ctrl+šípky (so Shift jemnejšie)", "V priblíženej fotografii posúvať pohľad."],
      ["Shift+← / Shift+→", "V paneli Kalendár prejsť na predchádzajúci alebo nasledujúci deň či mesiac s fotografiami."],
      ["Ctrl+A / Ctrl+C", "V paneli Vlastnosti vybrať všetky riadky / kopírovať vybrané riadky."], ["Enter / Esc v dialógoch", "Podľa dialógu potvrdiť alebo zrušiť zadávanie."]
    ] }
  ] }
];
