//! Windows shell icons, loaded off the UI thread and shared by file type.
use std::{collections::HashMap, path::Path, sync::Arc};

use image::RgbaImage;

use crate::search::Entry;

pub type Cache = HashMap<String, Option<Arc<RgbaImage>>>;

pub fn key(entry: &Entry) -> String {
    if entry.folder {
        return "folder:".into();
    }
    let extension = Path::new(&entry.name)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "exe" | "lnk" | "ico" | "cur" | "scr" | "cpl" | "url"
    ) {
        return format!("path:{}", entry.path().display());
    }
    format!("ext:{extension}")
}

pub fn fetch(entry: &Entry) -> Option<Arc<RgbaImage>> {
    #[cfg(windows)]
    {
        shell::fetch(entry, key(entry).starts_with("path:"))
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        None
    }
}

#[cfg(test)]
pub fn load(rows: &[Entry], mut cache: Cache) -> Cache {
    // ponytail: batch eviction above 1024 icons; use LRU only if cache misses become costly.
    if cache.len() > 1024 {
        cache.clear();
    }
    #[cfg(windows)]
    let _com = shell::Com::new();
    #[cfg(windows)]
    let factory = shell::factory();
    for entry in rows {
        let key = key(entry);
        cache.entry(key.clone()).or_insert_with(|| {
            #[cfg(windows)]
            {
                shell::load(entry, key.starts_with("path:"), factory.as_ref()?)
            }
            #[cfg(not(windows))]
            {
                None
            }
        });
    }
    cache
}

#[cfg(windows)]
mod shell {
    use std::{mem::size_of, os::windows::ffi::OsStrExt, ptr};

    use image::RgbaImage;
    use windows::{
        Win32::{
            Graphics::Imaging::*,
            Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL},
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::{Shell::*, WindowsAndMessaging::*},
        },
        core::PCWSTR,
    };

    use super::*;

    pub struct Com(bool);

    impl Com {
        pub fn new() -> Self {
            Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    pub fn factory() -> Option<IWICImagingFactory> {
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }.ok()
    }

    #[allow(clippy::missing_const_for_thread_local)]
    pub fn fetch(entry: &Entry, own_icon: bool) -> Option<Arc<RgbaImage>> {
        thread_local! {
            static LOCAL: std::cell::RefCell<Option<(Com, Option<IWICImagingFactory>)>> = const { std::cell::RefCell::new(None) };
        }
        LOCAL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let local = slot.get_or_insert_with(|| (Com::new(), factory()));
            local
                .1
                .as_ref()
                .and_then(|factory| load(entry, own_icon, factory))
        })
    }

    pub fn load(
        entry: &Entry,
        own_icon: bool,
        factory: &IWICImagingFactory,
    ) -> Option<Arc<RgbaImage>> {
        let path = if own_icon {
            entry.path()
        } else {
            entry.name.clone().into()
        };
        let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let attributes = if entry.folder {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        let flags = SHGFI_ICON
            | SHGFI_SMALLICON
            | if own_icon {
                SHGFI_FLAGS(0)
            } else {
                SHGFI_USEFILEATTRIBUTES
            };
        let mut info = SHFILEINFOW::default();
        unsafe {
            if SHGetFileInfoW(
                PCWSTR(path.as_ptr()),
                attributes,
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                flags,
            ) == 0
            {
                return None;
            }
            let image = (|| {
                let bitmap = factory.CreateBitmapFromHICON(info.hIcon).ok()?;
                let (mut width, mut height) = (0, 0);
                bitmap.GetSize(&mut width, &mut height).ok()?;
                if width == 0 || height == 0 || width > 1024 || height > 1024 {
                    return None;
                }
                // WIC handles legacy icon masks and converts premultiplied pixels to straight BGRA.
                let converter = factory.CreateFormatConverter().ok()?;
                converter
                    .Initialize(
                        &bitmap,
                        &GUID_WICPixelFormat32bppBGRA,
                        WICBitmapDitherTypeNone,
                        None,
                        0.,
                        WICBitmapPaletteTypeCustom,
                    )
                    .ok()?;
                let mut pixels = vec![0; (width * height * 4) as usize];
                converter
                    .CopyPixels(ptr::null(), width * 4, &mut pixels)
                    .ok()?;
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.swap(0, 2); // BGRA -> RGBA
                }
                Some(Arc::new(RgbaImage::from_raw(width, height, pixels)?))
            })();
            let _ = DestroyIcon(info.hIcon);
            image
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_icons_and_cache() {
        let entry = |name: &str, folder| Entry {
            name: name.into(),
            directory: String::new(),
            size: None,
            modified: String::new(),
            folder,
        };
        let mut rows = vec![
            entry("sample.txt", false),
            entry("SAMPLE.TXT", false),
            entry("folder", true),
        ];
        let executable = std::env::current_exe().unwrap();
        rows.push(Entry {
            name: executable.file_name().unwrap().to_string_lossy().into(),
            directory: executable.parent().unwrap().to_string_lossy().into(),
            ..entry("", false)
        });
        assert_eq!(key(&rows[0]), key(&rows[1]));
        assert_ne!(key(&rows[0]), key(&rows[2]));
        assert_ne!(key(&entry("one.exe", false)), key(&entry("two.exe", false)));
        let cache = load(&rows, Cache::new());
        #[cfg(windows)]
        {
            for row in &rows {
                let icon = cache[&key(row)].as_ref().expect("Windows shell icon");
                assert_eq!(
                    icon.as_raw().len(),
                    (icon.width() * icon.height() * 4) as usize
                );
                assert!(icon.as_raw().chunks_exact(4).any(|pixel| pixel[3] != 0));
            }
            assert_ne!(
                cache[&key(&rows[0])].as_ref().unwrap().as_raw(),
                cache[&key(&rows[2])].as_ref().unwrap().as_raw()
            );
        }
        let full = (0..1025).map(|index| (index.to_string(), None)).collect();
        assert!(load(&[], full).is_empty());
        let again = load(&rows, cache.clone());
        if let Some(icon) = cache[&key(&rows[0])].as_ref() {
            assert!(Arc::ptr_eq(icon, again[&key(&rows[0])].as_ref().unwrap()));
        }
    }
}
