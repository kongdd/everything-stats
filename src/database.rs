// ESDb layout verified by ../docs/everything-db-format.typ and inspect_db.py.
use std::{fs, path::Path};

use anyhow::{Context, Result, bail, ensure};

pub(crate) struct Directory {
    pub parent: Option<usize>,
    pub path: Vec<u8>,
    pub direct: i64,
    pub recursive: i64,
}

pub(crate) struct Counts {
    pub nodes: Vec<Directory>,
    pub total: i64,
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(size)
            .context("database offset overflow")?;
        let bytes = self
            .data
            .get(self.position..end)
            .with_context(|| format!("truncated database at 0x{:x}", self.position))?;
        self.position = end;
        Ok(bytes)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn integer(&mut self) -> Result<usize> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?) as usize)
    }

    fn packed(&mut self) -> Result<usize> {
        let byte = self.byte()?;
        if byte == 255 {
            self.integer()
        } else {
            Ok(byte as usize)
        }
    }

    fn string(&mut self) -> Result<&'a [u8]> {
        let size = self.packed()?;
        self.take(size)
    }

    fn name(&mut self, previous: &mut Vec<u8>) -> Result<()> {
        let suffix = self.packed()?;
        if suffix != 0 {
            let remove = self.packed()?;
            ensure!(remove <= previous.len(), "invalid name backtrack");
            previous.truncate(previous.len() - remove);
            previous.extend_from_slice(self.take(suffix)?);
        }
        Ok(())
    }
}

pub(crate) fn collect(path: &Path) -> Result<Counts> {
    let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&data)
}

pub(crate) fn parse(data: &[u8]) -> Result<Counts> {
    let mut r = Reader { data, position: 0 };
    ensure!(
        r.take(4)? == b"ESDb" && r.integer()? == 0x01070014,
        "only uncompressed ESDb 1.7.20 is supported"
    );
    let flags = r.integer()?;
    ensure!(flags & !0x7f3f == 0, "unknown flags 0x{flags:x}");
    let folders = r.integer()?;
    let files = r.integer()?;
    ensure!(
        folders <= data.len() / 4 && files <= data.len() / 5,
        "impossible record counts"
    );
    let mut types = Vec::new();
    for _ in 0..r.packed()? {
        let kind = r.packed()?;
        r.byte()?; // out_of_date
        match kind {
            0 => {
                for _ in 0..4 {
                    r.string()?;
                }
                r.take(16)?; // USN journal ID + next USN
            }
            2 => {
                r.string()?;
                r.take(8)?; // next_update
            }
            _ => bail!("source type {kind} has not been sample-validated"),
        }
        types.push(kind);
    }
    r.byte()?; // exclude_flags; already reflected in indexed records
    for _ in 0..3 {
        for _ in 0..r.packed()? {
            r.byte()?;
            r.string()?;
        }
    }
    let mut parents = Vec::with_capacity(folders);
    for _ in 0..folders {
        let parent = r.integer()?;
        ensure!(
            parent < folders + types.len(),
            "invalid folder parent/source"
        );
        parents.push(parent);
    }

    // Folder IDs are not topologically ordered. Resolve sources and reject cycles.
    let mut roots = vec![usize::MAX; folders];
    let mut order = Vec::with_capacity(folders);
    for start in 0..folders {
        let mut chain = Vec::new();
        let mut node = start;
        while node < folders && roots[node] == usize::MAX {
            roots[node] = usize::MAX - 1;
            chain.push(node);
            node = parents[node];
        }
        ensure!(
            node >= folders || roots[node] != usize::MAX - 1,
            "folder hierarchy cycle"
        );
        let source = if node < folders {
            roots[node]
        } else {
            node - folders
        };
        for node in chain.into_iter().rev() {
            roots[node] = source;
            order.push(node); // Reuse this parent-first order for paths and counts.
        }
    }
    let common = 8 * (flags & 0x0e).count_ones() as usize + 4 * usize::from(flags & 0x10 != 0);
    let folder_width = common + 8 * usize::from(flags & 0x20 != 0);
    let file_width = common + 8 * usize::from(flags & 1 != 0);
    let mut names = Vec::with_capacity(folders);
    let mut previous = Vec::new();
    for &source in &roots {
        r.name(&mut previous)?;
        ensure!(!previous.is_empty(), "empty folder name");
        names.push(previous.clone()); // Keep raw WTF-8 bytes, including lone surrogates.
        r.take(folder_width)?;
        if types[source] == 0 {
            r.take(8)?;
        } // NTFS directory FRN only
    }
    let mut direct = vec![0_i64; folders];
    previous.clear(); // Names start a new differential stream for files.
    for _ in 0..files {
        let parent = r.integer()?;
        ensure!(parent < folders, "file must reference an indexed folder");
        r.name(&mut previous)?;
        r.take(file_width)?;
        direct[parent] += 1;
    }
    for kind in [0, 3] {
        let count = r.integer()?;
        ensure!(
            count == roots.iter().filter(|&&s| types[s] == kind).count(),
            "unexpected type-{kind} FRN index count"
        );
        for _ in 0..count {
            let id = r.integer()?;
            ensure!(
                id < folders && types[roots[id]] == kind,
                "invalid FRN index"
            );
        }
    }
    for (is_folder, count) in [(true, folders), (false, files)] {
        for bit in [0x2000, 0x100, 0x200, 0x400, 0x800, 0x1000, 0x4000] {
            if flags & bit == 0
                || (is_folder && (bit == 0x4000 || (bit == 0x100 && flags & 0x20 == 0)))
            {
                continue;
            }
            for _ in 0..count {
                ensure!(r.integer()? < count, "invalid sort index");
            }
        }
    }
    ensure!(
        r.position == data.len(),
        "unparsed tail at 0x{:x}",
        r.position
    );

    // Use native DB IDs: no path interning, re-parsing, or file-path materialization.
    let mut nodes: Vec<_> = names
        .into_iter()
        .enumerate()
        .map(|(id, path)| Directory {
            parent: (parents[id] < folders).then_some(parents[id]),
            path,
            direct: direct[id],
            recursive: direct[id],
        })
        .collect();
    for &id in &order {
        if let Some(parent) = nodes[id].parent {
            let mut path = nodes[parent].path.clone();
            if !path.ends_with(b"\\") {
                path.push(b'\\');
            }
            path.extend(std::mem::take(&mut nodes[id].path));
            nodes[id].path = path;
        } // Root names already include their drive; no source path prefix.
    }
    for id in order.into_iter().rev() {
        if let Some(parent) = nodes[id].parent {
            nodes[parent].recursive += nodes[id].recursive;
        }
    }
    Ok(Counts {
        nodes,
        total: files as i64,
    })
}
