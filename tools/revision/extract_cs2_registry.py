#!/usr/bin/env python3
"""Recover constant x86-64 CS2 registrations from a local initializer export.

Requires Capstone. Original bytes and output evidence stay outside Git. This
extracts registrations, not command semantics or general executable behavior.
Unknown values, repeated writes and incomplete rows are errors.
"""
import argparse
import hashlib
import json
from pathlib import Path


def extract(raw, options):
    import capstone
    from capstone.x86_const import X86_OP_IMM, X86_OP_REG, X86_OP_MEM, X86_REG_RIP

    bits_per_byte = 8
    pointer_bytes = 8
    opcode_bytes = 2
    width_bytes = 1
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    decoder.detail = True
    families = {}
    for family, aliases in (
        ("rax", "rax eax ax al ah"), ("rbx", "rbx ebx bx bl bh"),
        ("rcx", "rcx ecx cx cl ch"), ("rdx", "rdx edx dx dl dh"),
        ("rsi", "rsi esi si sil"), ("rdi", "rdi edi di dil"),
        ("rbp", "rbp ebp bp bpl"), ("rsp", "rsp esp sp spl"),
    ):
        for alias in aliases.split():
            families[alias] = family
    for number in range(8, 16):
        for suffix in ("", "d", "w", "b"):
            families[f"r{number}{suffix}"] = f"r{number}"
    registers, writes, entries = {}, [], {}

    def register(identifier):
        name = decoder.reg_name(identifier)
        return families.get(name, name)

    def value(operand):
        if operand.type == X86_OP_IMM:
            return operand.imm
        if operand.type == X86_OP_REG:
            known = registers.get(register(operand.reg))
            if known is not None:
                return known & ((1 << (operand.size * bits_per_byte)) - 1)
        return None

    def address(operand, instruction):
        memory = operand.mem
        if memory.base == X86_REG_RIP and not memory.index:
            return instruction.address + instruction.size + memory.disp
        base = registers.get(register(memory.base)) if memory.base else 0
        index = registers.get(register(memory.index)) if memory.index else 0
        if base is None or index is None:
            return None
        return base + index * memory.scale + memory.disp

    decoded_bytes = 0
    field_widths = {
        options.handler_offset: pointer_bytes,
        options.id_offset: opcode_bytes,
        options.width_offset: width_bytes,
    }
    instructions = list(decoder.disasm(raw, options.code_base))
    destinations = {operand.imm for instruction in instructions if instruction.mnemonic.startswith("j")
                    for operand in instruction.operands if operand.type == X86_OP_IMM}
    for instruction in instructions:
        if instruction.address in destinations:
            registers.clear()
        decoded_bytes += instruction.size
        operands = instruction.operands
        destination, known = None, None
        if instruction.mnemonic == "lea" and len(operands) == 2 and operands[0].type == X86_OP_REG:
            destination, known = register(operands[0].reg), address(operands[1], instruction)
        elif instruction.mnemonic in ("mov", "movabs") and len(operands) == 2:
            if operands[0].type == X86_OP_REG:
                destination, known = register(operands[0].reg), value(operands[1])
            elif operands[0].type == X86_OP_MEM:
                target = address(operands[0], instruction)
                if target is not None and options.table_base <= target < options.table_end:
                    opcode, field = divmod(target - options.table_base, options.entry_stride)
                    known_value = value(operands[1])
                    if known_value is None or field_widths.get(field) != operands[0].size:
                        raise ValueError(f"unresolved registration at {instruction.address:#x}")
                    row = entries.setdefault(opcode, {})
                    if field in row:
                        raise ValueError(f"repeated registration of opcode {opcode}, field {field}")
                    fact = {"instruction": hex(instruction.address), "address": hex(target),
                            "opcode": opcode, "field": field, "width": operands[0].size,
                            "value": known_value & ((1 << (operands[0].size * bits_per_byte)) - 1)}
                    row[field] = fact
                    writes.append(fact)
        elif instruction.mnemonic == "xor" and len(operands) == 2 and operands[0].type == X86_OP_REG and operands[1].type == X86_OP_REG and operands[0].reg == operands[1].reg:
            destination, known = register(operands[0].reg), 0
        for identifier in instruction.regs_access()[1]:
            registers.pop(register(identifier), None)
        # Partial register writes do not establish a full address value.
        if destination is not None and known is not None and operands[0].size >= 4:
            registers[destination] = known & ((1 << (operands[0].size * bits_per_byte)) - 1)
        if instruction.mnemonic == "call":
            for name in "rax rcx rdx rsi rdi r8 r9 r10 r11".split():
                registers.pop(name, None)
        if instruction.mnemonic.startswith("j"):
            registers.clear()
    if decoded_bytes != len(raw):
        raise ValueError("initializer export was not completely disassembled")
    count, remainder = divmod(options.table_end - options.table_base, options.entry_stride)
    if remainder or set(entries) != set(range(count)):
        raise ValueError("incomplete registration table")
    for opcode, row in entries.items():
        if set(row) != set(field_widths) or row[options.id_offset]["value"] != opcode:
            raise ValueError(f"incomplete or inconsistent registration for opcode {opcode}")
        if row[options.width_offset]["value"] not in (0, 1):
            raise ValueError(f"invalid operand width for opcode {opcode}")
    return {"table": hex(options.table_base), "stride": options.entry_stride,
            "extent": hex(options.table_end), "initializer_sha256": hashlib.sha256(raw).hexdigest(),
            "writes": writes, "unknown": [], "entries": entries}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hex-input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    for name in ("code-base", "table-base", "table-end", "entry-stride", "handler-offset", "id-offset", "width-offset"):
        parser.add_argument(f"--{name}", type=lambda text: int(text, 0), required=True)
    options = parser.parse_args()
    if options.entry_stride <= 0 or options.table_end <= options.table_base:
        parser.error("invalid registration table bounds")
    raw = bytes.fromhex(options.hex_input.read_text())
    result = extract(raw, options)
    with options.output.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(f"Recovered {len(result['entries'])} complete registrations")


if __name__ == "__main__":
    main()
