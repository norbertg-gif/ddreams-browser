use crate::{db::Db, formats::FormatTable};
use serde::Serialize;
use std::{collections::{BTreeMap, HashMap}, sync::{Arc, Mutex}};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DateSource { Taken, Database, Modified }

impl DateSource {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value { "taken" => Ok(Self::Taken), "database" => Ok(Self::Database), "modified" => Ok(Self::Modified), _ => Err(format!("Unknown calendar date source: {value}")) }
    }
    pub fn as_str(self) -> &'static str { match self { Self::Taken => "taken", Self::Database => "database", Self::Modified => "modified" } }
    pub fn column(self) -> &'static str { match self { Self::Taken => "a.exif_date", Self::Database => "a.acdsee_date", Self::Modified => "a.file_modified" } }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAggregate { pub date_source: String, pub dated_count: i64, pub undated_count: i64, pub years: Vec<CalendarYear> }
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarYear { pub year: i32, pub count: i64, pub months: Vec<CalendarMonth> }
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarMonth { pub month: u32, pub count: i64, pub days: Vec<CalendarDay> }
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarDay { pub day: u32, pub date: String, pub count: i64, pub hours: Vec<CalendarHour> }
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarHour { pub hour: u32, pub count: i64 }

#[derive(Default)]
struct MutableDay { count: i64, hours: BTreeMap<u32, i64> }

#[derive(Default)]
struct CalendarCache {
    revision: Option<(u64, i64)>,
    entries: HashMap<DateSource, Arc<CalendarAggregate>>,
}

pub struct CalendarService { db: Db, formats: Arc<FormatTable>, cache: Mutex<CalendarCache> }

impl CalendarService {
    pub fn new(db: Db, formats: Arc<FormatTable>) -> Arc<Self> { Arc::new(Self { db, formats, cache: Mutex::new(CalendarCache::default()) }) }

    pub fn aggregate(&self, source: DateSource) -> Result<Arc<CalendarAggregate>, String> {
        let conn = self.db.lock().map_err(|_| "catalog lock poisoned")?;
        let version = conn.query_row("PRAGMA main.data_version", [], |row| row.get(0)).map_err(|e| e.to_string())?;
        let revision = (conn.total_changes(), version);
        let mut cache = self.cache.lock().map_err(|_| "calendar cache poisoned")?;
        if cache.revision != Some(revision) {
            cache.entries.clear();
            cache.revision = Some(revision);
        }
        if conn.is_autocommit() {
            if let Some(value) = cache.entries.get(&source) { return Ok(value.clone()); }
        }
        let column = source.column();
        let index_name = format!("idx_assets_listed_{}_ext", column.strip_prefix("a.").unwrap_or(column));
        let index_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)", [&index_name], |row| row.get(0)
        ).map_err(|e| e.to_string())?;
        let access = if index_exists { format!("INDEXED BY {index_name}") } else { "NOT INDEXED".into() };
        let media = self.formats.sql_in("a.ext_lower", &["image", "raw", "video"]);
        let sql = format!("SELECT {column} FROM assets a {access} WHERE a.is_listed=1 AND {media}");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0)).map_err(|e| e.to_string())?;
        let mut undated_count = 0;
        let mut tree: BTreeMap<i32, BTreeMap<u32, BTreeMap<u32, MutableDay>>> = BTreeMap::new();
        for row in rows {
            let timestamp = row.map_err(|e| e.to_string())?;
            let Some(timestamp) = timestamp else { undated_count += 1; continue; };
            let Some(date) = timestamp.get(..10) else { undated_count += 1; continue; };
            let Some((year, month, day)) = parse_day(date) else { undated_count += 1; continue; };
            let hour = timestamp.get(11..13).and_then(|value| value.parse::<u32>().ok()).filter(|value| *value < 24).unwrap_or(0);
            let entry = tree.entry(year).or_default().entry(month).or_default().entry(day).or_default();
            entry.count += 1;
            *entry.hours.entry(hour).or_default() += 1;
        }
        let mut years = Vec::with_capacity(tree.len());
        let mut dated_count = 0;
        for (year, months) in tree {
            let mut year_count = 0;
            let mut output_months = Vec::with_capacity(months.len());
            for (month, days) in months {
                let mut month_count = 0;
                let output_days = days.into_iter().map(|(day, value)| {
                    month_count += value.count;
                    CalendarDay { day, date: format!("{year:04}-{month:02}-{day:02}"), count: value.count, hours: value.hours.into_iter().map(|(hour, count)| CalendarHour { hour, count }).collect() }
                }).collect::<Vec<_>>();
                year_count += month_count;
                output_months.push(CalendarMonth { month, count: month_count, days: output_days });
            }
            dated_count += year_count;
            years.push(CalendarYear { year, count: year_count, months: output_months });
        }
        let value = Arc::new(CalendarAggregate { date_source: source.as_str().into(), dated_count, undated_count, years });
        if conn.is_autocommit() { cache.entries.insert(source, value.clone()); }
        Ok(value)
    }
}

fn parse_day(value: &str) -> Option<(i32, u32, u32)> {
    if value.len() != 10 || value.as_bytes()[4] != b'-' || value.as_bytes()[7] != b'-' { return None; }
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if parts.next().is_some() || year < 1900 || !(1..=12).contains(&month) || day == 0 || day > days[(month - 1) as usize] { return None; }
    Some((year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, assets_page, next_date, AssetFilter};
    use rusqlite::Connection;
    use std::time::Instant;

    fn fixture() -> (Db, Arc<FormatTable>) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT,ext TEXT,ext_lower TEXT,is_listed INTEGER,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER);CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER);CREATE INDEX idx_assets_exif_date ON assets(exif_date);INSERT INTO assets(id,name,ext,ext_lower,is_listed,size,exif_date,acdsee_date,file_modified) VALUES(1,'a.jpg','jpg','jpg',1,10,'2020-06-01T09:10:00','2020-06-02T10:00:00','2020-06-03T11:00:00'),(2,'b.dng','dng','dng',1,20,'2020-06-01T15:40:00','2020-06-02T16:00:00',NULL),(3,'c.mp4','mp4','mp4',1,30,'2020-06-02T12:00:00','1601-01-01T00:00:00','2020-06-02T12:00:00'),(4,'d.doc','doc','doc',1,40,'2020-06-01T09:00:00','2020-06-01T09:00:00','2020-06-01T09:00:00'),(5,'e.jpg','jpg','jpg',1,50,'1899-12-31T00:00:00',NULL,'1601-01-01T00:00:00'),(6,'f.jpg','jpg','jpg',1,60,NULL,NULL,NULL),(7,'g.jpg','jpg','jpg',0,70,'2020-06-01T00:00:00','2020-06-01T00:00:00','2020-06-01T00:00:00');").unwrap();
        (Arc::new(Mutex::new(conn)), Arc::new(FormatTable::parse(include_str!("../formats.json")).unwrap()))
    }
    fn filter(source: &str, from: &str, to: &str) -> AssetFilter {
        AssetFilter { kind:"date".into(),id:None,root_id:None,include_children:false,folder_path:None,ids:None,value:None,query:None,sort:"name".into(),descending:false,offset:0,limit:100,rating_mode:None,rating_value:None,label_filter:None,tagged_only:false, category_filter: None, keyword_filter: None,file_group:None,include_categories:false,include_keywords:false,date_from:Some(from.into()),date_to:Some(to.into()),date_source:Some(source.into()),date_no_date:false }
    }
    #[test]
    fn date_ranges_use_each_source_and_keep_end_date_inclusive() {
        let (db, formats) = fixture();
        for (source, expected) in [("taken", 3), ("database", 2), ("modified", 1)] {
            let page = assets_page(&db, &filter(source,"2020-06-01","2020-06-02"),&formats).unwrap();
            assert_eq!(page.total, expected, "{source}");
        }
    }
    #[test]
    fn cache_refreshes_after_local_write_and_external_commit() {
        let path = std::env::temp_dir().join(format!("ddreams-calendar-{}-{:?}.sqlite", std::process::id(), std::thread::current().id()));
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY,is_listed INTEGER,ext_lower TEXT,exif_date TEXT);
                              INSERT INTO assets VALUES(1,1,'jpg','2025-06-01');").unwrap();
        let reader = Arc::new(Mutex::new(rusqlite::Connection::open(&path).unwrap()));
        let formats = Arc::new(FormatTable::parse(include_str!("../formats.json")).unwrap());
        let service = CalendarService::new(reader.clone(), formats);
        let first = service.aggregate(DateSource::Taken).unwrap();
        assert_eq!(first.dated_count, 1);
        assert!(Arc::ptr_eq(&first, &service.aggregate(DateSource::Taken).unwrap()));
        writer.execute("INSERT INTO assets VALUES(2,1,'jpg','2025-06-02')", []).unwrap();
        assert_eq!(service.aggregate(DateSource::Taken).unwrap().dated_count, 2);
        reader.lock().unwrap().execute("UPDATE assets SET is_listed=0", []).unwrap();
        assert_eq!(service.aggregate(DateSource::Taken).unwrap().dated_count, 0);
        drop((service, reader, writer));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn aggregate_counts_month_days_hours_undated_and_excludes_bogus_dates() {
        let (db, formats) = fixture(); let service = CalendarService::new(db, formats);
        let aggregate = service.aggregate(DateSource::Taken).unwrap();
        assert_eq!(aggregate.dated_count, 3); assert_eq!(aggregate.undated_count, 2);
        assert_eq!(aggregate.years[0].months[0].count,3);
        assert_eq!(aggregate.years[0].months[0].days[0].date,"2020-06-01");
        assert_eq!(aggregate.years[0].months[0].days[0].count,2);
        assert_eq!(aggregate.years[0].months[0].days[0].hours.iter().map(|h|h.count).sum::<i64>(),2);
    }
    #[test]
    fn aggregate_uses_date_covering_index_when_available() {
        let (db, formats) = fixture();
        db.lock().unwrap().execute("CREATE INDEX idx_assets_listed_exif_date_ext ON assets(is_listed,exif_date,ext_lower)", []).unwrap();
        let aggregate = CalendarService::new(db, formats).aggregate(DateSource::Taken).unwrap();
        assert_eq!(aggregate.dated_count, 3);
        assert_eq!(aggregate.undated_count, 2);
    }
    #[test]
    fn no_date_view_matches_aggregate_no_date_count_and_days_sum_to_total() {
        let (db, formats) = fixture(); let service = CalendarService::new(db.clone(), formats.clone());
        let aggregate = service.aggregate(DateSource::Taken).unwrap();
        let mut no_date=filter("taken","2020-01-01","2020-01-01");no_date.date_no_date=true;
        assert_eq!(assets_page(&db,&no_date,&formats).unwrap().total,aggregate.undated_count);
        let year=filter("taken","2020-01-01","2020-12-31");
        let total=assets_page(&db,&year,&formats).unwrap().total;
        let day_sum=aggregate.years.iter().flat_map(|y|&y.months).flat_map(|m|&m.days).map(|d|d.count).sum::<i64>();
        assert_eq!(total,day_sum);
    }
    #[test]
    fn date_filter_rejects_pre_1900_and_bad_date_values() {
        assert!(next_date("2020-02-29").is_some()); assert_eq!(next_date("2021-02-29"),None);
        assert_eq!(parse_day("2020-02-31"), None);
        assert_eq!(DateSource::parse("taken").unwrap(),DateSource::Taken);
        assert!(DateSource::parse("random").is_err());
    }
    #[test]
    #[ignore = "uses the real read-only catalog and prints calendar source totals and cache timings"]
    fn bench_real_catalog_calendar_aggregates() {
        let path=db::catalog_path(); let conn=db::open_read_only(&path).unwrap(); let db=Arc::new(Mutex::new(conn)); let formats=FormatTable::load(); let service=CalendarService::new(db,formats);
        for source in [DateSource::Taken,DateSource::Database,DateSource::Modified] {
            let start=Instant::now();let value=service.aggregate(source).unwrap();let cold=start.elapsed();
            let cached_start=Instant::now();let cached=service.aggregate(source).unwrap();let cached_time=cached_start.elapsed();
            let june=value.years.iter().filter(|y|y.year==2020).flat_map(|y|&y.months).filter(|m|m.month==6).map(|m|m.count).sum::<i64>();
            println!("calendar {}: dated={}, undated={}, all={}, June 2020={}, cold={cold:?}, cached={cached_time:?}",source.as_str(),value.dated_count,value.undated_count,value.dated_count+value.undated_count,june);
            assert!(Arc::ptr_eq(&value,&cached));
        }
    }
}
