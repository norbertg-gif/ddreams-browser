struct TestFs {
    failed_restores: std::sync::Mutex<HashSet<PathBuf>>,
    force_cross: Option<bool>,
    fail_move: AtomicUsize,
    move_calls: AtomicUsize,
    recycle: PathBuf,
    copied_mtime_offset: AtomicI64,
    recycled: std::sync::Mutex<std::collections::HashMap<PathBuf, PathBuf>>,
}

impl TestFs {
    fn recycled_item(&self, original: &Path) -> Option<PathBuf> {
        self.recycled
            .lock()
            .unwrap()
            .get(original)
            .filter(|p| p.exists())
            .cloned()
    }
}

impl FileSystemOps for TestFs {
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }
    fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }
    fn is_symlink(&self, p: &Path) -> bool {
        NativeFileSystem.is_symlink(p)
    }
    fn metadata(&self, p: &Path) -> io::Result<fs::Metadata> {
        fs::metadata(p)
    }
    fn same_volume(&self, a: &Path, b: &Path) -> bool {
        self.force_cross.unwrap_or_else(|| volume_key(a) == volume_key(b))
    }
    fn move_no_replace(&self, a: &Path, b: &Path) -> io::Result<()> {
        let n = self.move_calls.fetch_add(1, Ordering::SeqCst);
        if n == self.fail_move.load(Ordering::SeqCst) {
            return Err(io::Error::other("simulated failure"));
        }
        NativeFileSystem.move_no_replace(a, b)
    }
    fn copy_no_replace(&self, a: &Path, b: &Path) -> io::Result<()> {
        NativeFileSystem.copy_no_replace(a, b)?;
        let offset = self.copied_mtime_offset.load(Ordering::SeqCst);
        if offset != i64::MIN {
            let modified = fs::metadata(a)?.modified()?;
            let rounded = if offset < 0 {
                modified
                    .checked_sub(std::time::Duration::from_secs(offset.unsigned_abs()))
                    .unwrap()
            } else {
                modified
                    .checked_add(std::time::Duration::from_secs(offset as u64))
                    .unwrap()
            };
            File::options()
                .write(true)
                .open(b)?
                .set_times(FileTimes::new().set_modified(rounded))?;
        }
        Ok(())
    }
    fn remove_file(&self, p: &Path) -> io::Result<()> {
        NativeFileSystem.remove_file(p)
    }
    fn create_dir(&self, p: &Path) -> io::Result<()> {
        NativeFileSystem.create_dir(p)
    }
    fn remove_dir(&self, p: &Path) -> io::Result<()> {
        NativeFileSystem.remove_dir(p)
    }
    fn read_dir(&self, p: &Path) -> io::Result<Vec<PathBuf>> {
        NativeFileSystem.read_dir(p)
    }
    fn set_modified(&self, p: &Path, t: SystemTime) -> io::Result<()> {
        NativeFileSystem.set_modified(p, t)
    }
    fn recycle_delete(&self, paths: &[PathBuf]) -> Result<(), String> {
        for path in paths {
            let to = self.recycle.join(format!(
                "$R{}-{}",
                self.move_calls.fetch_add(1, Ordering::SeqCst),
                path.file_name().unwrap().to_string_lossy()
            ));
            NativeFileSystem.move_no_replace(path, &to).map_err(|e| e.to_string())?;
            self.recycled.lock().unwrap().insert(path.clone(), to);
        }
        Ok(())
    }
    fn has_recycle_bin(&self, path: &Path) -> bool {
        !path.to_string_lossy().contains("nobin")
    }
    fn restore_recycled(&self, original: &Path) -> Result<(), String> {
        if self.failed_restores.lock().unwrap().contains(original) {
            return Err("simulated restore failure".into());
        }
        let data = self.recycled_item(original).ok_or("not in bin")?;
        NativeFileSystem
            .move_no_replace(&data, original)
            .map_err(|e| e.to_string())?;
        self.recycled.lock().unwrap().remove(original);
        Ok(())
    }
}

fn temp() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "ddreams-fileops-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn db() -> Db {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO meta VALUES('catalog_modified','0');CREATE TABLE ddreams_changes(id INTEGER PRIMARY KEY,ts TEXT,action TEXT,summary TEXT,undone INTEGER DEFAULT 0);CREATE TABLE ddreams_change_items(change_id INTEGER,table_name TEXT,row_key TEXT,column_name TEXT,old_value TEXT,new_value TEXT);CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT,path TEXT);CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT,path TEXT);CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty INTEGER,is_orphan INTEGER,path TEXT);CREATE TABLE categories(id INTEGER PRIMARY KEY,name TEXT,parent_id INTEGER,root_id INTEGER);CREATE TABLE keywords(id INTEGER PRIMARY KEY,parent_id INTEGER,name TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));INSERT INTO folder_roots VALUES(1,'R:','R:\\');INSERT INTO folders VALUES(1,NULL,1,'R:','R:\\');INSERT INTO folders VALUES(2,1,1,'src','R:\\src');INSERT INTO folders VALUES(3,1,1,'dst','R:\\dst');INSERT INTO assets VALUES(1,2,'a.jpg','jpg','jpg',1,3,10,20,24,'cap','notes','author',4,1,'Red','2020-01-01T00:00:00Z','2020-01-02',NULL,0,0,'R:\\src\\a.jpg');INSERT INTO categories VALUES(1,'Cat',NULL,1);INSERT INTO keywords VALUES(1,NULL,'Key');INSERT INTO asset_categories VALUES(1,1);INSERT INTO asset_keywords VALUES(1,1);").unwrap();
    Arc::new(Mutex::new(c))
}

fn manager(_root: &Path, readonly: bool, fs: Arc<dyn FileSystemOps>) -> FileManager {
    let db = db();
    let formats = crate::formats::FormatTable::load_selected(None);
    let listing = ListingService::new(db.clone(), formats);
    let writer = Arc::new(CatalogWriter::new(db.clone(), !readonly));
    FileManager::with_fs(db, listing, writer, readonly, fs)
}

fn set_up(root: &Path, force_cross: Option<bool>) -> (Arc<TestFs>, FileManager) {
    let src = root.join("src");
    let dst = root.join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.jpg"), b"abc").unwrap();
    let recycle = root.join("recycle");
    fs::create_dir_all(&recycle).unwrap();
    let disk = Arc::new(TestFs {
        failed_restores: Default::default(),
        force_cross,
        fail_move: AtomicUsize::new(usize::MAX),
        move_calls: AtomicUsize::new(0),
        recycle,
        copied_mtime_offset: AtomicI64::new(i64::MIN),
        recycled: Default::default(),
    });
    let manager = manager(root, false, disk.clone());
    (disk, manager)
}

fn seed_asset_path(db: &Db, root: &Path) {
    let path = root.join("src/a.jpg");
    db.lock()
        .unwrap()
        .execute(
            "UPDATE assets SET path=?1 WHERE id=1",
            [path.to_string_lossy().to_string()],
        )
        .unwrap();
}

fn quarantine_fixture(root: &Path) -> (Arc<TestFs>, FileManager, crate::quarantine::Settings) {
    let (disk, manager) = set_up(root, None);
    seed_asset_path(&manager.db, root);
    crate::schema::migrate_catalog(&mut manager.db.lock().unwrap()).unwrap();
    let settings = crate::quarantine::Settings::fixture(&root.join("src"), &root.join("dst"));
    manager.sync_incoming(settings.incoming().to_str().unwrap()).unwrap();
    (disk, manager, settings)
}

fn fail_history(db: &Db) {
    db.lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_history BEFORE INSERT ON ddreams_changes
             BEGIN SELECT RAISE(ABORT,'simulated catalogue failure'); END;",
        )
        .unwrap();
}
