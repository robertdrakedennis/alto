"""Retain every ordered actual PLAYER/NPC publication; never trim the RTR."""
import hashlib
import json

TRACE_MARKER = b"[trace-info] "


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON property {key}")
        result[key] = value
    return result


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique_object,
                      parse_constant=lambda value: require(False, f"nonfinite {value}"))


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False,
                      allow_nan=False, separators=(",", ":")).encode("utf8")


def record_digest(result, value):
    result.update(canonical(value) + b"\n")


def native_publications(world_log, destination):
    actor_ids, pids = set(), set()
    counts = {"player_info": 0, "npc_info": 0}
    lineage = hashlib.sha256()
    with world_log.open("rb") as source, destination.open("xb") as output:
        for ordinal, line in enumerate(source, start=1):
            if TRACE_MARKER not in line:
                continue
            raw = line.split(TRACE_MARKER, maxsplit=1)[1]
            require(raw.endswith(b"\n"), "partial native publication line")
            row = decode(raw)
            kind = row.get("t")
            require(kind in counts, f"unknown native publication {kind}")
            require(type(row.get("pid")) is int, "native publication pid")
            pids.add(row["pid"])
            if kind == "npc_info":
                require(isinstance(row.get("npcs"), list), "full NPC publication roster")
                for actor in row["npcs"]:
                    require(isinstance(actor, list) and len(actor) == 5
                            and all(type(value) is int for value in actor), "native NPC tuple")
                    actor_ids.add(actor[0])
            else:
                require(all(type(row.get(field)) is int for field in ("x", "z", "level", "angle")),
                        "native physical PLAYER publication")
            counts[kind] += 1
            record_digest(lineage, [ordinal, row])
            output.write(raw)
    require(all(counts.values()), "both native publication kinds are required")
    return actor_ids, pids, {**counts, "orderedPublicationLineageSha256": lineage.hexdigest()}
