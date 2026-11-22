// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <algorithm>
#include <cstdlib>
#include <string>
#include <vector>
#include "common/arch.h"
#include "common/logging/log.h"
#include "core/guest_trace.h"
#include "core/red_zone.h"

#if defined(_WIN32) && defined(ARCH_X86_64)
#include <windows.h>

// Diagnostic only: logs when the guest reaches chosen addresses of the eboot.
//
//   SHADPS4_GUEST_TRACE=371DF20:rdi+544/4:rdi+350/1,371F350,35D03B0
//
// Entries are separated by commas. Each is the address in the eboot (hex, as in the ELF),
// optionally followed by values to read when it is reached: register+offset/size in hex.
// Each address is a breakpoint: when hit, the original byte is put back, that one instruction is
// single-stepped and the breakpoint is set again.

namespace Core::GuestTrace {

namespace {

struct Peek {
    int reg;
    s64 offset;
    u32 size;
    std::string text;
};

struct Point {
    u64 vaddr;
    u8* address;
    u8 original;
    u32 hits;
    std::vector<Peek> peeks;
};

constexpr u32 MaxLoggedHits = 60;
constexpr const char* RegNames[] = {"rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
                                    "r8",  "r9",  "r10", "r11", "r12", "r13", "r14", "r15"};

std::vector<Point> points;
u64 eboot_base = 0;
u64 eboot_size = 0;
thread_local Point* pending = nullptr;

u64 RegValue(const CONTEXT& c, int reg) {
    return (&c.Rax)[reg];
}

bool ReadGuest(u64 address, void* out, size_t size) {
    SIZE_T read = 0;
    return ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<void*>(address), out, size,
                             &read) &&
           read == size;
}

void WriteByte(u8* address, u8 value) {
    *address = value;
    FlushInstructionCache(GetCurrentProcess(), address, 1);
}

std::string Where(u64 address) {
    if (address >= eboot_base && address < eboot_base + eboot_size) {
        return fmt::format("{:#x}", address - eboot_base);
    }
    return fmt::format("[{:#x}]", address);
}

void ParsePeek(const std::string& text, Point& point) {
    const auto slash = text.find('/');
    const auto sign = text.find_first_of("+-");
    const std::string reg_name = text.substr(0, std::min(sign, slash));
    Peek peek{-1, 0, 8, text};
    for (int i = 0; i < 16; ++i) {
        if (reg_name == RegNames[i]) {
            peek.reg = i;
        }
    }
    if (peek.reg < 0) {
        return;
    }
    if (sign != std::string::npos) {
        peek.offset = std::strtoll(text.substr(sign + 1).c_str(), nullptr, 16);
        if (text[sign] == '-') {
            peek.offset = -peek.offset;
        }
    }
    if (slash != std::string::npos) {
        peek.size = std::min<u32>(8, std::strtoul(text.substr(slash + 1).c_str(), nullptr, 16));
    }
    point.peeks.push_back(peek);
}

void LogHit(Point& point, const CONTEXT& c) {
    ++point.hits;
    if (point.hits > MaxLoggedHits) {
        return;
    }
    std::string values;
    for (const auto& peek : point.peeks) {
        u64 value = 0;
        if (ReadGuest(RegValue(c, peek.reg) + peek.offset, &value, peek.size)) {
            values += fmt::format(" {}={:#x}", peek.text, value);
        } else {
            values += fmt::format(" {}=?", peek.text);
        }
    }
    // Who called: the return address when at a function's entry, then the frame pointer chain.
    std::string callers;
    u64 return_address = 0;
    if (ReadGuest(c.Rsp, &return_address, 8)) {
        callers += " " + Where(return_address);
    }
    u64 frame = c.Rbp;
    for (int depth = 0; depth < 8 && frame != 0 && (frame & 7) == 0; ++depth) {
        u64 link[2]{};
        if (!ReadGuest(frame, link, sizeof(link))) {
            break;
        }
        callers += " " + Where(link[1]);
        if (link[0] <= frame) {
            break;
        }
        frame = link[0];
    }
    LOG_CRITICAL(Debug,
                 "GuestTrace {:#x} hit {} rdi={:#x} rsi={:#x} rdx={:#x} rax={:#x}{} | from{}",
                 point.vaddr, point.hits, c.Rdi, c.Rsi, c.Rdx, c.Rax, values, callers);
}

} // namespace

void Install(u64 base, u64 size) {
    const char* spec = std::getenv("SHADPS4_GUEST_TRACE");
    if (spec == nullptr || *spec == '\0') {
        return;
    }
    eboot_base = base;
    eboot_size = size;
    const std::string text = spec;
    size_t start = 0;
    while (start < text.size()) {
        const size_t end = std::min(text.find(',', start), text.size());
        const std::string entry = text.substr(start, end - start);
        start = end + 1;
        size_t field_end = std::min(entry.find(':'), entry.size());
        Point point{};
        point.vaddr = std::strtoull(entry.substr(0, field_end).c_str(), nullptr, 16);
        if (point.vaddr == 0 || point.vaddr >= size) {
            LOG_ERROR(Debug, "GuestTrace: address outside the eboot: {}", entry);
            continue;
        }
        while (field_end < entry.size()) {
            const size_t next = std::min(entry.find(':', field_end + 1), entry.size());
            ParsePeek(entry.substr(field_end + 1, next - field_end - 1), point);
            field_end = next;
        }
        point.address = reinterpret_cast<u8*>(RedZone::CurrentAddress(base + point.vaddr));
        points.push_back(std::move(point));
    }
    // Set the breakpoints only once the list is final: the handler keeps pointers into it.
    for (auto& point : points) {
        DWORD old_protection = 0;
        if (!VirtualProtect(point.address, 1, PAGE_EXECUTE_READWRITE, &old_protection)) {
            LOG_ERROR(Debug, "GuestTrace: cannot write at {:#x}", point.vaddr);
            point.address = nullptr;
            continue;
        }
        point.original = *point.address;
        WriteByte(point.address, 0xCC);
        LOG_CRITICAL(Debug, "GuestTrace: watching {:#x} (original byte {:#x})", point.vaddr,
                     point.original);
    }
}

bool HandleException(void* exception_pointers) {
    if (points.empty()) {
        return false;
    }
    auto* exception = static_cast<EXCEPTION_POINTERS*>(exception_pointers);
    auto& context = *exception->ContextRecord;
    const DWORD code = exception->ExceptionRecord->ExceptionCode;
    if (code == EXCEPTION_SINGLE_STEP) {
        if (pending == nullptr) {
            return false;
        }
        WriteByte(pending->address, 0xCC);
        pending = nullptr;
        return true;
    }
    if (code != EXCEPTION_BREAKPOINT) {
        return false;
    }
    auto* address = static_cast<u8*>(exception->ExceptionRecord->ExceptionAddress);
    for (auto& point : points) {
        if (point.address != address) {
            continue;
        }
        context.Rip = reinterpret_cast<u64>(address);
        LogHit(point, context);
        WriteByte(point.address, point.original);
        if (point.hits >= MaxLoggedHits) {
            // Nothing more to log here: leave the original code in place, at full speed.
            return true;
        }
        context.EFlags |= 0x100; // trap flag: come back after this one instruction
        pending = &point;
        return true;
    }
    return false;
}

} // namespace Core::GuestTrace

#else

namespace Core::GuestTrace {
void Install(u64, u64) {}
bool HandleException(void*) {
    return false;
}
} // namespace Core::GuestTrace

#endif
