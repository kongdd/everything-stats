"""Generate tiny ESDb samples in an isolated Everything instance (Windows only)."""
import os
import subprocess
from pathlib import Path

EXE = Path(os.environ["ProgramFiles"]) / "Everything" / "Everything.exe"
ES = EXE.with_name("es.exe")
ROOT = Path(__file__).resolve().parents[1] / "data" / "everything-1750"
INSTANCE = "es-stats-format-probe"


def snapshot(name, files=(), settings=None):
    ROOT.mkdir(parents=True, exist_ok=True)
    folder = ROOT / "fixture"
    folder.mkdir(exist_ok=True)
    for item in sorted(folder.rglob("*"), key=lambda path: len(path.parts), reverse=True):
        if item.is_dir():
            item.rmdir()
        else:
            item.unlink()
    for relative, size in files:
        target = folder / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(b"x" * size)
        os.utime(target, (1700000000, 1700000000))
    options = {
        "auto_include_fixed_volumes": 0,
        "auto_include_fixed_refs_volumes": 0,
        "auto_include_fixed_fat_volumes": 0,
        "auto_include_remote_volumes": 0,
        "index_size": 0,
        "index_folder_size": 0,
        "index_date_created": 0,
        "index_date_modified": 0,
        "index_date_accessed": 0,
        "index_attributes": 0,
        "fast_size_sort": 0,
        "fast_date_created_sort": 0,
        "fast_date_modified_sort": 0,
        "fast_date_accessed_sort": 0,
        "fast_attributes_sort": 0,
        "fast_extension_sort": 0,
        "index_recent_changes": 0,
        "journal_max_size": 0,
        "db_backup": 0,
        "db_save_on_exit": 0,
        "show_tray_icon": 0,
    }
    options.update(settings or {})
    config = ROOT / f"{name}.ini"
    database = ROOT / f"{name}.db"
    assert not database.exists(), f"Refusing to overwrite {database}"
    config.write_text("[Everything]\n" + "\n".join(f"{k}={v}" for k, v in options.items()))
    prefix = [str(EXE), "-instance", INSTANCE]
    process = subprocess.Popen(prefix + [
        "-config", str(config), "-db", str(database), "-no-auto-index",
        "-folders", str(folder), "-startup",
    ])
    try:
        count = subprocess.check_output([
            str(ES), "-instance", INSTANCE, "-timeout", "10000", "-get-result-count", "*",
        ], text=True, timeout=15).strip()
        expected = len(files) + 1 + len(list(folder.rglob("*/")))
        assert int(count) == expected, f"Unexpected probe index size: {count} != {expected}"
        subprocess.run(prefix + ["-save-db-now"], check=True, timeout=15)
        data = database.read_bytes()
        print(name, len(data), "bytes,", count, "records:", data.hex(" "))
    finally:
        subprocess.run(prefix + ["-exit", "-wait"], check=True, timeout=15)
        process.wait(timeout=15)


if __name__ == "__main__":
    cases = [
        ("empty", [], {}),
        ("one", [("a.txt", 3)], {}),
        ("two", [("a.txt", 3), ("b.txt", 7)], {}),
        ("size", [("a.txt", 3), ("b.txt", 7)], {"index_size": 1}),
        ("modified", [("a.txt", 3)], {"index_date_modified": 1}),
        ("names", [("A/same.txt", 0), ("B/same.txt", 1), ("汉" * 100 + ".txt", 2)], {}),
        ("counts128", [(f"d{i:03}/a.txt", i) for i in range(130)], {}),
        ("properties", [("A/a.txt", 127), ("B/b.txt", 128)], {
            "index_size": 1, "index_folder_size": 1, "index_date_created": 1,
            "index_date_modified": 1, "index_date_accessed": 1, "index_attributes": 1,
            "fast_size_sort": 1, "fast_date_created_sort": 1, "fast_date_modified_sort": 1,
            "fast_date_accessed_sort": 1, "fast_attributes_sort": 1, "fast_extension_sort": 1,
        }),
    ]
    for name, files, settings in cases:
        if not (ROOT / f"{name}.db").exists():
            snapshot(name, files, settings)
