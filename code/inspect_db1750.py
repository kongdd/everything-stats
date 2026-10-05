"""Read-only ESDb 1.7.50 inspection; intentionally rejects unverified source types."""
import argparse
import hashlib
import json
import zlib
from pathlib import Path


class Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def take(self, size):
        end = self.pos + size
        if size < 0 or end > len(self.data):
            raise ValueError(f"truncated field at 0x{self.pos:x}")
        result = self.data[self.pos:end]
        self.pos = end
        return result

    def integer(self, size, order="little"):
        return int.from_bytes(self.take(size), order)

    def varint(self):
        first = self.integer(1)
        count = 0
        while count < 8 and first & (0x80 >> count):
            count += 1
        payload = first & (0xff >> (count + 1))
        for byte in self.take(count):
            payload = (payload << 8) | byte
        result = payload + sum(1 << (7 * i) for i in range(1, count + 1))
        if result > 0xffffffffffffffff:
            raise ValueError("varint overflow")
        return result

    def short(self):
        value = self.integer(1)
        return value if value < 255 else 255 + self.varint()

    def string(self):
        return self.take(self.short())

    def date(self):
        first = self.take(1)
        return None if first == b"\xff" else int.from_bytes(first + self.take(7), "big")

    def name(self, previous):
        size, backtrack = self.short(), self.short()
        if backtrack > len(previous):
            raise ValueError(f"invalid name backtrack at 0x{self.pos:x}")
        return previous[:len(previous) - backtrack] + self.take(size)

    def attributes(self, flags, directory):
        values = {}
        if flags & (0x20 if directory else 1):
            size = self.varint()
            values["size"] = size - 1 if size else None
        for bit, key in [(2, "created"), (4, "modified"), (8, "accessed")]:
            if flags & bit:
                values[key] = self.date()
        if flags & 16:
            value = self.varint()
            values["attributes"] = value - 1 if value else None
        return values


def source(reader, kind):
    status = list(reader.take(2))
    stamp = reader.date()
    fields = [reader.string() for _ in range(5 if kind == 0 else 2)]
    mode = reader.integer(4)
    if kind == 0:
        root_id = reader.integer(8)
        if root_id:
            reader.string()
            reader.integer(8)
            reader.date()
        journal_id, next_usn = reader.integer(8), reader.integer(8)
        return {"kind": kind, "status": status, "stamp": stamp, "fields": fields,
                "drive_type": mode, "root_id": root_id,
                "journal_id": journal_id, "next_usn": next_usn}
    dates = [reader.date() for _ in range(4)]
    switches = list(reader.take(2))
    return {"kind": kind, "status": status, "stamp": stamp, "fields": fields,
            "update_type": mode, "dates": dates, "switches": switches}


def change_journal(reader):
    identifier = reader.integer(8)
    start, count = reader.varint(), reader.varint()
    for _ in range(count):
        action = reader.integer(1)
        if action > 10:
            raise ValueError(f"unverified journal action {action}")
        for _ in range(3):
            reader.date()
        reader.string()
        reader.string()
        if action in (1, 3, 4, 5, 6, 8, 9, 10):
            if action in (6, 8, 9, 10):
                reader.varint()
            for _ in range(3):
                reader.date()
            reader.varint()
        if action in (3, 8):
            reader.string()
        if action in (4, 9):
            reader.date()
            reader.string()
            reader.string()
    return {"id": identifier, "start": start, "count": count}


def inspect(path):
    data = path.read_bytes()
    if len(data) < 16 or data[:8] != b"ESDb\x32\x00\x07\x01":
        raise ValueError("expected uncompressed ESDb 1.7.50")
    if zlib.crc32(data[:-4]) != int.from_bytes(data[-4:], "big"):
        raise ValueError("CRC32 mismatch")
    reader = Reader(data[:-4])
    reader.take(8)
    flags = reader.integer(4)
    if flags & ~0x5f3f:
        raise ValueError(f"unverified flags 0x{flags:x}")
    # ponytail: only empty property configurations; decode custom properties before broadening.
    for _ in range(4):
        if reader.varint():
            raise ValueError("unverified nonempty property configuration")
    directory_count, file_count = reader.varint(), reader.varint()
    sources = []
    groups = []
    for group, kind in enumerate((None, 0, None, None, 2, None, None)):
        count = reader.short()
        groups.append(count)
        if count and kind is None:
            raise ValueError(f"unverified source group {group}")
        sources.extend(source(reader, kind) for _ in range(count))
    source_end = reader.pos
    exclude_flags = reader.integer(1)
    filters = []
    for _ in range(3):
        filters.append([(reader.integer(1), reader.string()) for _ in range(reader.short())])
    journal = change_journal(reader)
    parent_offset = reader.pos
    parents = [reader.varint() for _ in range(directory_count)]
    if any(parent >= directory_count + len(sources) for parent in parents):
        raise ValueError("invalid directory parent")
    kinds = [None] * directory_count
    for index in range(directory_count):
        chain, current = [], index
        while current < directory_count and kinds[current] is None:
            chain.append(current)
            if len(chain) > directory_count:
                raise ValueError("directory cycle")
            current = parents[current]
        if current < directory_count:
            kind = kinds[current]
        else:
            kind = sources[current - directory_count]["kind"]
        for item in chain:
            kinds[item] = kind
    directory_offset = reader.pos
    names, examples = [], []
    previous = b""
    for kind in kinds:
        previous = reader.name(previous)
        names.append(previous)
        values = reader.attributes(flags, True)
        if kind == 0:
            values["frn"] = reader.integer(8)
        if len(examples) < 5:
            examples.append({"name": previous, **values})
    file_offset = reader.pos
    previous = b""
    direct = [0] * directory_count
    file_examples = []
    for _ in range(file_count):
        parent = reader.varint()
        if parent >= directory_count:
            raise ValueError("unverified file parent referencing a source")
        direct[parent] += 1
        previous = reader.name(previous)
        values = reader.attributes(flags, False)
        if len(file_examples) < 5:
            file_examples.append({"parent": parent, "name": previous, **values})

    def record_path(index):
        parts = []
        while index < directory_count:
            parts.append(names[index])
            index = parents[index]
        return b"\\".join(reversed(parts))

    for index, example in enumerate(examples):
        example["path"] = record_path(index)
    for example in file_examples:
        example["path"] = record_path(example["parent"]) + b"\\" + example["name"]
    tail_offset = reader.pos
    frn_counts = []
    for _ in range(3):
        count = reader.varint()
        frn_counts.append(count)
        for _ in range(count):
            if reader.varint() >= directory_count:
                raise ValueError("invalid FRN index")
    sort_offset = reader.pos
    directory_sorts = ["path"]
    file_sorts = ["path"]
    for bit, key in [(0x100, "size"), (0x200, "created"), (0x400, "modified"),
                     (0x800, "accessed"), (0x1000, "attributes"), (0x4000, "extension")]:
        if flags & bit:
            file_sorts.append(key)
            if key != "extension" and (key != "size" or flags & 0x20):
                directory_sorts.append(key)
    for sorts, count in [(directory_sorts, directory_count), (file_sorts, file_count)]:
        for _ in sorts:
            seen = bytearray(count)
            for _ in range(count):
                index = reader.varint()
                if index >= count or seen[index]:
                    raise ValueError("invalid sort permutation")
                seen[index] = 1
    footer_offset = reader.pos
    # Empty property configurations omit the conditional property-cache block entirely.
    last_error = reader.string()
    stats = []
    for _ in range(7):
        count = reader.varint()
        values = [reader.varint() for _ in range(4)] if count else []
        stamp = reader.date() if count else None
        stats.append({"count": count, "values": values, "stamp": stamp})
    counters = [reader.varint() for _ in range(9)]
    if reader.pos != len(data) - 4:
        raise ValueError(f"unparsed bytes at 0x{reader.pos:x}")
    return {
        "version": "1.7.50", "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
        "crc32": f"{zlib.crc32(data[:-4]):08x}", "flags": f"0x{flags:x}",
        "directories": directory_count, "files": file_count, "source_groups": groups,
        "sources": sources, "exclude_flags": exclude_flags, "filters": filters,
        "journal": journal, "offsets": {
            "source_end": source_end, "parents": parent_offset, "directories": directory_offset,
            "files": file_offset, "frn_indexes": tail_offset,
            "sorts": sort_offset, "footer": footer_offset, "crc32": len(data) - 4,
        },
        "frn_counts": frn_counts, "directory_sorts": directory_sorts,
        "file_sorts": file_sorts,
        "footer": {"last_error": last_error, "stats": stats, "counters": counters},
        "tail_unparsed_bytes": len(data) - 4 - reader.pos,
        "directory_examples": examples, "file_examples": file_examples,
        "direct_total": sum(direct),
    }


def self_check():
    cases = {
        "00": 0, "7f": 127, "8000": 128, "803f": 191, "bfff": 16511,
        "c00000": 16512, "dfffff": 2113663, "e0000000": 2113664,
        "f000000000": 270549120, "ff0000000000000000": 72624976668147840,
    }
    for encoded, expected in cases.items():
        reader = Reader(bytes.fromhex(encoded))
        assert reader.varint() == expected
        assert reader.pos == len(reader.data)
    assert Reader(b"\xff\x80\x00").short() == 383
    assert Reader(b"\x00\x00").name(b"same") == b"same"
    assert Reader(bytes.fromhex("01da1747c66d0000")).date() == 133444736000000000
    assert Reader(b"\xff").date() is None
    for invalid in (b"\x80", b"\xff\xff\xff\xff\xff\xff\xff\xff\xff"):
        try:
            Reader(invalid).varint()
        except ValueError:
            continue
        raise AssertionError("invalid varint accepted")
    from tempfile import TemporaryDirectory

    source_bytes = b"\0" * 10 + b"\x02Q:\0" + b"\0" * 38
    body = (b"ESDb\x32\x00\x07\x01" + b"\0" * 8 + b"\x01\x01"
            + b"\0" * 4 + b"\x01" + source_bytes + b"\0" * 16
            + b"\x01\x02\x00Q:\x00\x05\x00a.txt" + b"\0" * 22)
    with TemporaryDirectory() as directory:
        path = Path(directory) / "test.db"
        path.write_bytes(body + zlib.crc32(body).to_bytes(4, "big"))
        result = inspect(path)
        assert (result["directories"], result["files"], result["direct_total"]) == (1, 1, 1)
        assert result["file_examples"][0]["path"] == b"Q:\\a.txt"
        assert result["tail_unparsed_bytes"] == 0
        path.write_bytes(body + b"\0" * 4)
        try:
            inspect(path)
        except ValueError as error:
            assert "CRC32" in str(error)
        else:
            raise AssertionError("invalid CRC32 accepted")
    print("encoding, record layout, exact EOF and CRC32 checks passed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=Path, nargs="?")
    parser.add_argument("--test", action="store_true")
    args = parser.parse_args()
    if args.test:
        self_check()
    elif args.database:
        print(json.dumps(inspect(args.database), ensure_ascii=True, indent=2,
                         default=lambda value: value.decode("utf-8", "surrogatepass")))
    else:
        parser.error("provide a database or --test")
