//! Everything 1.5: query indexed folder counts through the official SDK3.
use std::{
    collections::HashMap,
    ffi::{CString, c_void},
    ptr,
};

use anyhow::{Context, Result, ensure};

use crate::database::{Counts, Directory};

const PATH: u32 = 240;
const DIRECT: u32 = 156;
const RECURSIVE: u32 = 284;

type Pointer = *mut c_void;
type Release = unsafe extern "system" fn(Pointer) -> i32;

unsafe extern "system" {
    fn Everything3_ConnectW(instance: *const u16) -> Pointer;
    fn Everything3_DestroyClient(client: Pointer) -> i32;
    fn Everything3_CreateSearchState() -> Pointer;
    fn Everything3_DestroySearchState(search: Pointer) -> i32;
    fn Everything3_SetSearchTextUTF8(search: Pointer, text: *const u8) -> i32;
    fn Everything3_AddSearchPropertyRequest(search: Pointer, property: u32) -> i32;
    fn Everything3_Search(client: Pointer, search: Pointer) -> Pointer;
    fn Everything3_DestroyResultList(results: Pointer) -> i32;
    fn Everything3_GetResultListViewportCount(results: Pointer) -> usize;
    fn Everything3_GetResultPropertyTextUTF8(
        results: Pointer,
        index: usize,
        property: u32,
        buffer: *mut u8,
        capacity: usize,
    ) -> usize;
    fn Everything3_GetResultPropertySIZE_T(results: Pointer, index: usize, property: u32) -> usize;
}

// All SDK objects are released on both success and error paths.
struct Handle(Pointer, Release);

impl Handle {
    fn new(pointer: Pointer, release: Release) -> Result<Self> {
        ensure!(
            !pointer.is_null(),
            "Everything SDK3 failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self(pointer, release))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: each non-null SDK object is owned and released exactly once.
        unsafe { (self.1)(self.0) };
    }
}

pub(crate) fn collect(extensions: Option<&str>) -> Result<Counts> {
    // SAFETY: calls use owned SDK handles and appropriately sized output buffers.
    unsafe {
        let client = Handle::new(Everything3_ConnectW(ptr::null()), Everything3_DestroyClient)
            .context("start the default Everything 1.5 instance before querying its index")?;
        let search = Handle::new(
            Everything3_CreateSearchState(),
            Everything3_DestroySearchState,
        )?;
        let query = CString::new(extensions.map_or_else(
            || "folder:".to_string(),
            |extensions| format!("file: ext:{}", extensions.replace(',', ";")),
        ))?;
        let path_property = if extensions.is_some() { 1 } else { PATH }; // 1 = parent path.
        ensure!(
            Everything3_SetSearchTextUTF8(search.0, query.as_ptr().cast()) != 0,
            "cannot set SDK3 query"
        );
        let properties: &[u32] = if extensions.is_some() {
            &[1]
        } else {
            &[PATH, DIRECT, RECURSIVE]
        };
        for &property in properties {
            ensure!(
                Everything3_AddSearchPropertyRequest(search.0, property) != 0,
                "cannot request SDK3 property {property}"
            );
        }
        let results = Handle::new(
            Everything3_Search(client.0, search.0),
            Everything3_DestroyResultList,
        )?;
        let count = Everything3_GetResultListViewportCount(results.0);
        ensure!(count != usize::MAX, "cannot read SDK3 result count");
        let mut nodes = Vec::with_capacity(if extensions.is_none() { count } else { 0 });
        let mut direct = HashMap::new();
        for index in 0..count {
            let length = Everything3_GetResultPropertyTextUTF8(
                results.0,
                index,
                path_property,
                ptr::null_mut(),
                0,
            );
            ensure!(
                length > 0 && length < isize::MAX as usize,
                "invalid SDK3 path length"
            );
            // The size query includes NUL; the copy returns bytes excluding NUL.
            let mut path = vec![0; length];
            ensure!(
                Everything3_GetResultPropertyTextUTF8(
                    results.0,
                    index,
                    path_property,
                    path.as_mut_ptr(),
                    path.len()
                ) == length - 1,
                "cannot read SDK3 path"
            );
            path.truncate(length - 1);
            while path.last() == Some(&b'\\') {
                path.pop();
            }
            if extensions.is_some() {
                *direct.entry(path).or_insert(0_i64) += 1;
                continue;
            }
            nodes.push(Directory {
                parent: None,
                path,
                direct: i64::try_from(Everything3_GetResultPropertySIZE_T(
                    results.0, index, DIRECT,
                ))
                .context("invalid SDK3 direct file count")?,
                recursive: i64::try_from(Everything3_GetResultPropertySIZE_T(
                    results.0, index, RECURSIVE,
                ))
                .context("invalid SDK3 recursive file count")?,
            });
        }
        if extensions.is_some() {
            filtered_counts(direct)
        } else {
            hierarchy(nodes)
        }
    }
}

fn filtered_counts(direct: HashMap<Vec<u8>, i64>) -> Result<Counts> {
    let mut nodes = HashMap::new();
    for (path, count) in direct {
        let mut current = path.as_slice();
        loop {
            let node = nodes.entry(current.to_vec()).or_insert(Directory {
                parent: None,
                path: current.to_vec(),
                direct: 0,
                recursive: 0,
            });
            if current == path {
                node.direct += count;
            }
            node.recursive += count;
            let Some(end) = current.iter().rposition(|&byte| byte == b'\\') else {
                break;
            };
            // A UNC share is a root: do not invent \\server or empty ancestors.
            if current.starts_with(b"\\\\") && !current[2..end].contains(&b'\\') {
                break;
            }
            current = &current[..end];
        }
    }
    hierarchy(nodes.into_values().collect())
}

fn hierarchy(mut nodes: Vec<Directory>) -> Result<Counts> {
    let mut ids = HashMap::with_capacity(nodes.len());
    for (id, node) in nodes.iter().enumerate() {
        ensure!(!node.path.is_empty(), "empty SDK3 folder path");
        ensure!(
            ids.insert(node.path.clone(), id).is_none(),
            "duplicate SDK3 folder path"
        );
    }
    let mut total = 0_i64;
    for node in &mut nodes {
        node.parent = node
            .path
            .iter()
            .rposition(|&byte| byte == b'\\')
            .and_then(|end| ids.get(&node.path[..end]).copied());
        total = total
            .checked_add(node.direct)
            .context("SDK3 file count overflow")?;
    }
    let mut sums: Vec<_> = nodes.iter().map(|node| node.direct).collect();
    for node in &nodes {
        if let Some(parent) = node.parent {
            sums[parent] = sums[parent]
                .checked_add(node.recursive)
                .context("SDK3 file count overflow")?;
        }
    }
    ensure!(
        nodes
            .iter()
            .zip(sums)
            .all(|(node, sum)| node.recursive == sum),
        "SDK3 folder counts changed or contain missing parents; retry update"
    );
    Ok(Counts { nodes, total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_counts_include_ancestors_but_stop_at_volume_and_unc_roots() {
        let counts = filtered_counts(HashMap::from([
            (b"Q:\\A\\B".to_vec(), 2),
            (b"Q:\\A".to_vec(), 1),
            (b"Q:".to_vec(), 1),
            (br"\\server\share\docs".to_vec(), 3),
        ]))
        .unwrap();
        assert_eq!(counts.total, 7);
        let get = |path: &[u8]| counts.nodes.iter().find(|node| node.path == path).unwrap();
        assert_eq!(get(b"Q:").recursive, 4);
        assert_eq!(get(b"Q:\\A").direct, 1);
        assert_eq!(get(b"Q:\\A").recursive, 3);
        assert_eq!(get(br"\\server\share").recursive, 3);
        assert!(get(br"\\server\share").parent.is_none());
        assert_eq!(counts.nodes.len(), 5);
        assert!(filtered_counts(HashMap::new()).unwrap().nodes.is_empty());
    }

    #[test]
    fn resolves_child_before_parent_and_checks_counts() {
        let make = |path: &[u8], direct, recursive| Directory {
            path: path.to_vec(),
            parent: None,
            direct,
            recursive,
        };
        let counts = hierarchy(vec![make(b"Q:\\A", 2, 2), make(b"Q:", 1, 3)]).unwrap();
        assert_eq!(counts.nodes[0].parent, Some(1));
        assert_eq!(counts.total, 3);
        assert!(hierarchy(vec![make(b"Q:", 1, 2)]).is_err());
        assert!(hierarchy(vec![make(b"Q:", 1, 1), make(b"Q:", 0, 0)]).is_err());
    }
}
