use super::*;
use crate::database::{Counts, Directory};

fn options(recursive: bool, top: usize) -> StatsOptions {
    StatsOptions {
        command: None,
        path: None,
        top: NonZeroUsize::new(top).unwrap(),
        database: None,
        cache: Some("unused.db".into()),
        recursive,
    }
}

fn integer(data: &mut Vec<u8>, value: u32) {
    data.extend(value.to_le_bytes());
}

fn string(data: &mut Vec<u8>, text: &[u8]) {
    assert!(text.len() < 255);
    data.push(text.len() as u8);
    data.extend(text);
}

fn attributes(data: &mut Vec<u8>, flags: u32, folder: bool) {
    let size = if folder { 0x20 } else { 1 };
    for bit in [size, 2, 4, 8, 16] {
        if flags & bit != 0 {
            data.extend(vec![0; if bit == 16 { 4 } else { 8 }]);
        }
    }
}

fn fixture(kind: u8, flags: u32, files: u32) -> (Vec<u8>, usize) {
    let mut data = b"ESDb".to_vec();
    for value in [0x01070014, flags, 3, files] {
        integer(&mut data, value);
    }
    data.extend([1, kind, 0]); // One source, not stale.
    if kind == 0 {
        for text in [b"guid".as_slice(), b"Q:", b"", b""] {
            string(&mut data, text);
        }
        data.extend([0; 16]);
    } else {
        string(&mut data, b"Q:");
        data.extend([0; 8]);
    }
    data.extend([0; 4]); // exclude_flags + three empty filter lists.
    let parent_offset = data.len();
    for parent in [2, 3, 1] {
        integer(&mut data, parent);
    } // Child before parent.
    let mut previous_length = 0;
    for name in [b"B".as_slice(), b"Q:", b"A"] {
        data.extend([name.len() as u8, previous_length]);
        data.extend(name);
        previous_length = name.len() as u8;
        attributes(&mut data, flags, true);
        if kind == 0 {
            data.extend([0; 8]);
        }
    }
    previous_length = 0;
    for (id, parent) in [0, 0, 2, 1].into_iter().take(files as usize).enumerate() {
        integer(&mut data, parent);
        let name = if id < 2 {
            "中".as_bytes()
        } else {
            b"file.txt"
        };
        if id == 1 {
            data.push(0); // Repeated basename is still a separate indexed file record.
        } else {
            data.extend([name.len() as u8, previous_length]);
            data.extend(name);
            previous_length = name.len() as u8;
        }
        attributes(&mut data, flags, false);
    }
    integer(&mut data, if kind == 0 { 3 } else { 0 });
    if kind == 0 {
        for id in 0..3 {
            integer(&mut data, id);
        }
    }
    integer(&mut data, 0); // ReFS table.
    for (folder, count) in [(true, 3), (false, files)] {
        for bit in [0x2000, 0x100, 0x200, 0x400, 0x800, 0x1000, 0x4000] {
            if flags & bit != 0
                && !(folder && (bit == 0x4000 || (bit == 0x100 && flags & 0x20 == 0)))
            {
                for id in 0..count {
                    integer(&mut data, id);
                }
            }
        }
    }
    (data, parent_offset)
}

#[test]
fn parses_everything_and_caches_both_file_count_modes() {
    for kind in [0, 2] {
        let (data, _) = fixture(kind, 0x6525, 4);
        let counts = crate::database::parse(&data).unwrap();
        assert_eq!(counts.total, 4);
        assert_eq!(
            counts
                .nodes
                .iter()
                .find(|n| n.path == b"Q:")
                .unwrap()
                .direct,
            1
        );
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        cache::schema(&db).unwrap();
        cache::store(&mut db, b"snapshot", counts).unwrap();
        for (recursive, expected) in [(true, vec![4, 3, 2]), (false, vec![2, 1, 1])] {
            let rows = cache::ranked(&db, &options(recursive, 10), None).unwrap();
            assert_eq!(rows.iter().map(|r| r.count).collect::<Vec<_>>(), expected);
        }
        assert_eq!(cache::total(&db, b"Q:\\A").unwrap(), 3);
        assert_eq!(cache::total(&db, b"Q:\\missing").unwrap(), 0);
        let rows = cache::ranked(&db, &options(true, 1), Some(b"Q:\\A")).unwrap();
        assert_eq!(rows[0].path, b"Q:\\A");
        assert_eq!(rows[0].count, 3);
    }
    // Equal folder/file counts must not confuse which sort tables are present.
    assert_eq!(
        crate::database::parse(&fixture(2, 0x4101, 3).0)
            .unwrap()
            .total,
        3
    );
    assert_eq!(
        crate::database::parse(&fixture(2, 1, 0).0).unwrap().total,
        0
    );
}

#[test]
fn rejects_corruption_and_unrelated_sqlite_cache() {
    let (mut data, parent) = fixture(2, 0x6525, 4);
    assert!(crate::database::parse(&data[..data.len() - 1]).is_err());
    data[parent..parent + 4].copy_from_slice(&0_u32.to_le_bytes());
    assert!(crate::database::parse(&data).is_err());
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE important(data TEXT)")
        .unwrap();
    assert!(cache::schema(&db).is_err());
}

#[test]
fn update_checks_source_timestamp_without_taking_the_path() {
    use clap::Parser;
    let update = StatsOptions::parse_from(["es-stats", "update", "--db", "x.db"]);
    assert!(matches!(
        update.command,
        Some(Command::Update { force: false })
    ));
    let forced = StatsOptions::parse_from(["es-stats", "update", "-f"]);
    assert!(matches!(
        forced.command,
        Some(Command::Update { force: true })
    ));
    assert!(update.path.is_none());
    assert_eq!(update.database.unwrap(), PathBuf::from("x.db"));
    let query = StatsOptions::parse_from(["es-stats", r"C:\Users"]);
    assert!(query.command.is_none());
    assert_eq!(utc(std::time::UNIX_EPOCH).unwrap(), "1970-01-01 00:00:00Z");
}

#[test]
#[cfg(windows)]
fn cache_defaults_to_user_directory_and_accepts_lowercase_drives() {
    use clap::Parser;
    let options = StatsOptions::parse_from(["es-stats", "e:\\", "-n", "10"]);
    assert!(options.cache.is_none());
    assert_eq!(
        default_cache().unwrap(),
        PathBuf::from(env::var_os("LOCALAPPDATA").unwrap()).join("es-stats/stats.db")
    );
    assert_eq!(
        absolute_path(options.path.as_deref().unwrap()).unwrap(),
        b"E:"
    );
    let explicit = StatsOptions::parse_from(["es-stats", "--cache", "custom.db"]);
    assert_eq!(explicit.cache, Some(PathBuf::from("custom.db")));
}

#[test]
fn windows_paths_and_raw_name_bytes_are_preserved() {
    assert!(is_root(b"C:"));
    assert!(is_root(b"\\\\server\\share"));
    assert!(!is_root(b"\\\\server\\share\\a"));
    #[cfg(windows)]
    assert_eq!(
        absolute_path(Path::new("C:\\missing\\a\\..\\b\\")).unwrap(),
        b"C:\\missing\\b"
    );
    let path = b"C:\\raw\xed\xa0\x80";
    let counts = Counts {
        nodes: vec![
            Directory {
                parent: None,
                path: b"C:".to_vec(),
                direct: 0,
                recursive: 2,
            },
            Directory {
                parent: Some(0),
                path: path.to_vec(),
                direct: 2,
                recursive: 2,
            },
        ],
        total: 2,
    };
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    cache::schema(&db).unwrap();
    cache::store(&mut db, b"s", counts).unwrap();
    assert_eq!(
        cache::ranked(&db, &options(false, 1), None).unwrap()[0].path,
        path
    );
    assert_eq!(separated(1234567), "1,234,567");
}

#[test]
fn failed_replacement_keeps_previous_cache_intact() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    cache::schema(&db).unwrap();
    let counts = crate::database::parse(&fixture(2, 1, 4).0).unwrap();
    cache::store(&mut db, b"old", counts).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail BEFORE INSERT ON counts BEGIN SELECT RAISE(ABORT, 'fail'); END;",
    )
    .unwrap();
    let replacement = crate::database::parse(&fixture(2, 1, 4).0).unwrap();
    assert!(cache::store(&mut db, b"new", replacement).is_err());
    let (snapshot, total) = cache::metadata(&db).unwrap().unwrap();
    assert_eq!(snapshot, b"old");
    assert_eq!(total, 4);
}

#[test]
fn replaces_owned_v1_cache_transactionally() {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(&format!(
        "CREATE TABLE selections(id INTEGER, snapshot BLOB);
        CREATE TABLE directories(id INTEGER, path BLOB);
        CREATE TABLE counts(selection INTEGER, directory INTEGER);
        PRAGMA application_id={}; PRAGMA user_version=1;",
        0x45535453
    ))
    .unwrap();
    cache::schema(&db).unwrap();
    assert!(cache::metadata(&db).unwrap().is_none());
    let counts = crate::database::parse(&fixture(2, 1, 4).0).unwrap();
    cache::store(&mut db, b"new", counts).unwrap();
    assert_eq!(cache::metadata(&db).unwrap().unwrap(), (b"new".to_vec(), 4));
    cache::schema(&db).unwrap();
}
