import numpy as np

MODEL_NAME = "ViT-L-16-SigLIP2-256"
DIMENSION = 1024
SCHEMA_VERSION = "1"
COLOR_VERSION = 1
QUALITY_VERSION = 1
DECODER_VERSION = 2
IMAGE_RETRY_EXTENSIONS = frozenset({"heic", "heif", "hif", "avif", "jp2", "jpc", "j2k"})
RAW_EXTENSIONS = frozenset({
    "3fr", "arq", "arw", "bay", "cap", "cr2", "cr3", "crw", "cs1", "dcr", "dcs",
    "dng", "drf", "eip", "erf", "fff", "gpr", "iiq", "k25", "kdc", "mdc", "mef",
    "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "ptx", "pxn", "r3d", "raf",
    "raw", "rw2", "rwl", "rwz", "sr2", "srf", "srw", "x3f",
})
RAW_RETRY_ERRORS = frozenset({
    "LibRawNoThumbnailError", "LibRawUnsupportedThumbnailError", "UnidentifiedImageError",
})
RAW_MIN_EMBEDDED_PX = 800
FACE_VERSION = 1
FACE_IMAGE_MAX = 2048
FACE_MIN_SIDE = 28
FACE_MATCH_SIM = 0.45
FACE_STRONG_SIM = 0.60
FACE_TOP_K = 5
FACE_CLUSTER_MAX = 60000
FACE_CLUSTER_SIM = 0.55
FACE_CLUSTER_MIN_SIZE = 4
FACE_CLUSTER_GUESS_SIM = 0.35
FACE_TEMPLATE = np.array([[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]], dtype=np.float64)
TEXT_PROB_FLOOR = 0.003
TEXT_PROB_RELATIVE = 0.05
SIMILARITY_COSINE_FLOOR = 0.60
SUGGEST_K = 10
MIN_SCORE = 0.30
AUTO_GROUP_SCORE = 0.70
DUP_SAME = 0.95  # candidate level; the app keeps only same capture time / base name
DUP_SIMILAR = 0.93
SUGGEST_BATCH_CHUNK = 128
CLUSTER_MIN_SIZE = 8
CLUSTER_MIN_SIM = 0.72
CLUSTER_IGNORE_SIM = 0.95  # centroids of related groups easily reach 0.90
CLUSTER_ITERATIONS = 12
MAP_ITERATIONS = 15
MAP_RADIUS_SCALE = 0.94  # share of the packed circle that photos fill
MAP_PACK_DENSITY = 0.55
MAP_PACK_GAP = 1.04
MAP_PACK_ITERATIONS = 400
MAP_TSNE_ITERATIONS = 600
MAP_TSNE_EXAGGERATION = 150
MAP_TSNE_SEED = 7
MAP_GOLDEN_ANGLE = 2.39996323
CLUSTER_VOCABULARY = tuple(dict.fromkeys((
    "krajina", "hory", "kopce", "údolie", "les", "stromy", "lúka", "pole", "záhrada", "park",
    "more", "pláž", "pobrežie", "ostrov", "útes", "rieka", "jazero", "vodopád", "prameň", "močiar",
    "západ slnka", "východ slnka", "obloha", "oblaky", "dúha", "búrka", "blesk", "hmla", "noc", "hviezdy",
    "sneh", "zima", "jar", "leto", "jeseň", "ľad", "voda", "oheň", "púšť", "skaly",
    "mesto", "dedina", "ulica", "námestie", "architektúra", "kostol", "hrad", "zámok", "most", "veža",
    "interiér", "dom", "byt", "izba", "kuchyňa", "múzeum", "galéria", "pamiatky", "ruiny", "schody",
    "portrét", "deti", "rodina", "svadba", "oslava", "stretnutie", "ľudia", "skupina", "silueta", "móda",
    "koncert", "hudba", "festival", "divadlo", "umenie", "tanec", "šport", "futbal", "cyklistika", "beh",
    "pes", "mačka", "vtáky", "dravce", "kačice", "zvieratá", "kone", "hmyz", "motýle", "včely",
    "kvety", "ruže", "rastliny", "huby", "ovocie", "zelenina", "jedlo", "dezert", "káva", "víno",
    "auto", "motocykel", "bicykel", "vlak", "lietadlo", "loď", "autobus", "doprava", "cestovanie", "kempovanie",
    "makro", "detail", "textúra", "abstrakcia", "čiernobiela fotografia", "odraz", "tieň", "svetlá", "ohnisko", "panoráma",
    "dokumenty", "snímky obrazovky", "technika", "počítač", "telefón", "knihy", "hračky", "nástroje", "nábytok", "značky",
    "počasie", "príroda", "zvieratká", "akvárium", "plazy", "zoo", "farmárstvo", "vinica", "olivový háj", "sad",
    "turistika", "lyžovanie", "plávanie", "vodné športy", "lezenie", "preteky", "medaily", "výlet", "stan", "karavan",
    "jedlo na tanieri", "pečivo", "ryby", "morské živočíchy", "lastúry", "kamene", "kvapky", "okno", "dvere", "fasáda",
    "fontána", "socha", "pomník", "cintorín", "trh", "obchod", "reštaurácia", "hotel", "letisko", "prístav",
    "silvester", "vianoce", "veľká noc", "narodeniny", "krst", "promócia", "školská fotografia", "pracovisko", "kancelária", "ulica v noci",
)))
KEYWORD_VOCABULARY = tuple(dict.fromkeys(CLUSTER_VOCABULARY + (
    "slon", "nosorožec", "hroch", "žirafa", "zebra", "lev", "tiger", "leopard",
    "gepard", "medveď", "vlk", "líška", "jeleň", "srna", "diviak", "zajac",
    "veverička", "opica", "gorila", "panda", "ťava", "lama", "ovca", "koza",
    "krava", "prasa", "páv", "sova", "orol", "sokol", "bocian", "volavka",
    "plameniak", "papagáj", "labuť", "hus", "holub", "tučniak", "vrabec", "sýkorka",
    "krokodíl", "had", "jašterica", "korytnačka", "žaba", "delfín", "veľryba", "žralok",
    "vážka", "motýlica", "modlivka", "pavúk", "pavučina", "lienka", "čmeliak",
    "osa", "mravec", "chrobák", "koník", "slimák", "húsenica", "mucha",
    "orchidea", "tulipán", "púpava", "sedmokráska", "slnečnica", "levanduľa", "lekno",
    "mäta", "papraď", "mach", "lišajník", "kaktus", "šiška", "lístie", "kôra stromu",
    "rosa", "námraza", "snehová vločka", "kvapka vody", "špliechanie vody", "dážď",
    "dúhové odlesky", "protisvetlo", "bokeh", "dlhá expozícia", "ohňostroj", "Mesiac",
    "ľadovec", "jaskyňa", "skalný oblúk", "hrebeň hôr", "duny", "savana", "džungľa",
    "lesná cesta", "potok", "rybník", "priehrada", "vinice", "alej", "kvitnúci strom",
    "maják", "železnica", "stanica", "tunel", "električka", "ulička", "mrakodrap",
    "okná", "vitráž", "historická budova", "kamenný múr", "drevenica", "nádvorie",
    "skleník", "jesenné lístie", "mraky", "nočná obloha", "vidiek", "panoráma mesta",
)))
BATCH_SIZE = 32
DECODE_THREADS = 6
CHROMATIC_COLORS = ("červená", "oranžová", "hnedá", "žltá", "zelená", "tyrkysová", "modrá", "fialová", "ružová")
NEUTRAL_COLORS = ("biela", "sivá", "čierna")
COLOR_PALETTE = CHROMATIC_COLORS + NEUTRAL_COLORS + ("čiernobiela",)
COLOR_DEFAULTS = {color: 0.45 if color in NEUTRAL_COLORS else 0.06 for color in COLOR_PALETTE}
