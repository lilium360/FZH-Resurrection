// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <fstream>
#include <limits>
#include <memory>
#include <mutex>
#include <shared_mutex>
#include <string>
#include <thread>
#include <tuple>
#include <utility>
#include <vector>
#include "common/arch.h"
#include "common/logging/log.h"
#include "core/red_zone.h"

#if defined(_WIN32) && defined(ARCH_X86_64)
#include <windows.h>
#include <Zydis/Zydis.h>
#include <xbyak/xbyak.h>
#include "core/cpu_patches.h"

namespace Core::RedZone {

namespace {

constexpr s32 RedZoneSize = 128;
constexpr s32 MaxShift = 4096;
constexpr s32 NoFrame = std::numeric_limits<s32>::min();
constexpr u32 LeaRspSize = 8;
constexpr u64 TrampolineSpan = 8ULL << 20; // what module.cpp maps after every module

enum class Flow : u8 { None, Ret, Jmp, Jcc, Stop };
enum class JumpForm : u8 { Direct, Register, Memory, Table, ShortOnly };
enum class SpEffect : u8 { None, Add, Leave, Unsupported };
enum class FpEffect : u8 { None, Set, Pop, Kill };
enum class MemKind : u8 { None, Sp, Bp, Rip, Other, Odd };
enum class Verdict : u8 { None, Frame, Move, Left };

struct Insn {
    u32 offset;
    u8 length;
    Flow flow;
    JumpForm form;
    SpEffect sp_effect;
    FpEffect fp_effect;
    MemKind mem_kind;
    bool mem_indexed;
    bool mem_lea;
    bool rsp_value;
    bool padding;
    bool push;
    bool pop;
    bool modelled;
    bool reencodable;
    bool rip_relative;
    bool plain_branch;
    bool keeps_address; // reads the TLS base or is an instruction the emulator patches when it runs
    u8 modrm_offset;
    u8 sib_offset;
    u8 disp_offset;
    u8 disp_size;
    u16 mem_size;
    s32 sp_delta;
    s64 mem_disp;
    u64 target; // relative to the module
};

struct State {
    s32 sp;
    s32 fp;
    bool seen;
};

using Range = std::pair<s32, s32>;

struct Workspace {
    ZydisDecoder decoder;
    std::vector<Insn> insns;
    std::vector<s32> index_of;
    std::vector<State> states;
    std::vector<std::tuple<u32, s32, s32>> work;
    std::vector<Range> frame_ranges;
    std::vector<Range> red_ranges;
    std::vector<Range> kept_ranges;
    std::vector<s32> pushed;
    std::vector<s32> popped;
    std::vector<u32> leave_offsets;

    Workspace() {
        ZydisDecoderInit(&decoder, ZYDIS_MACHINE_MODE_LONG_64, ZYDIS_STACK_WIDTH_64);
    }
};

struct Function {
    u32 start;
    u32 size;
};

struct Result {
    u32 start;
    u32 size;
    Verdict verdict;
    bool indexed_only;
    s32 lowest;
    s32 shift;
    const char* reason;
};

struct Image {
    const u8* data;
    u64 size;
};

bool IsFamily(ZydisRegister reg, ZydisRegister full) {
    return reg != ZYDIS_REGISTER_NONE &&
           ZydisRegisterGetLargestEnclosing(ZYDIS_MACHINE_MODE_LONG_64, reg) == full;
}

void Note(const char*& slot, const char* why) {
    if (slot == nullptr) {
        slot = why;
    }
}

/// Reduces an instruction to what the stack analysis and the copy need.
Insn Summarize(const ZydisDecodedInstruction& z, const ZydisDecodedOperand* ops, const u8* bytes,
               u64 address) {
    Insn s{};
    s.length = z.length;
    s.modelled = true;
    const auto mnemonic = z.mnemonic;

    switch (z.meta.category) {
    case ZYDIS_CATEGORY_RET:
        s.flow = Flow::Ret;
        s.modelled &= mnemonic == ZYDIS_MNEMONIC_RET;
        break;
    case ZYDIS_CATEGORY_UNCOND_BR:
        s.flow = Flow::Jmp;
        if (z.meta.branch_type == ZYDIS_BRANCH_TYPE_FAR || mnemonic != ZYDIS_MNEMONIC_JMP) {
            s.modelled = false;
        } else if (ops[0].type == ZYDIS_OPERAND_TYPE_IMMEDIATE) {
            s.form = JumpForm::Direct;
            s.target = address + z.length + ops[0].imm.value.s;
            s.plain_branch = (z.length == 2 && bytes[0] == 0xEB) ||
                             (z.length == 5 && bytes[0] == 0xE9);
        } else if (ops[0].type == ZYDIS_OPERAND_TYPE_REGISTER) {
            s.form = JumpForm::Register;
        } else if (ops[0].type == ZYDIS_OPERAND_TYPE_MEMORY) {
            const auto& mem = ops[0].mem;
            s.form = mem.index != ZYDIS_REGISTER_NONE || IsFamily(mem.base, ZYDIS_REGISTER_RSP)
                         ? JumpForm::Table
                         : JumpForm::Memory;
        } else {
            s.modelled = false;
        }
        break;
    case ZYDIS_CATEGORY_COND_BR:
        s.flow = Flow::Jcc;
        if (ops[0].type != ZYDIS_OPERAND_TYPE_IMMEDIATE || mnemonic == ZYDIS_MNEMONIC_XBEGIN) {
            s.modelled = false;
            break;
        }
        s.target = address + z.length + ops[0].imm.value.s;
        switch (mnemonic) {
        case ZYDIS_MNEMONIC_JCXZ:
        case ZYDIS_MNEMONIC_JECXZ:
        case ZYDIS_MNEMONIC_JRCXZ:
        case ZYDIS_MNEMONIC_LOOP:
        case ZYDIS_MNEMONIC_LOOPE:
        case ZYDIS_MNEMONIC_LOOPNE:
            s.form = JumpForm::ShortOnly;
            break;
        default:
            s.form = JumpForm::Direct;
            s.plain_branch = (z.length == 2 && (bytes[0] & 0xF0) == 0x70) ||
                             (z.length == 6 && bytes[0] == 0x0F && (bytes[1] & 0xF0) == 0x80);
            break;
        }
        break;
    default:
        break;
    }
    if (mnemonic == ZYDIS_MNEMONIC_XBEGIN) {
        s.modelled = false;
    }
    if (mnemonic == ZYDIS_MNEMONIC_UD2 || mnemonic == ZYDIS_MNEMONIC_INT3 ||
        mnemonic == ZYDIS_MNEMONIC_HLT) {
        s.flow = Flow::Stop;
    }
    s.padding = mnemonic == ZYDIS_MNEMONIC_NOP || s.flow == Flow::Stop;
    if (s.flow != Flow::Jmp && s.flow != Flow::Jcc &&
        (z.raw.imm[0].is_relative || z.raw.imm[1].is_relative)) {
        s.modelled = false;
    }

    const bool lea_rsp = mnemonic == ZYDIS_MNEMONIC_LEA &&
                         ops[0].type == ZYDIS_OPERAND_TYPE_REGISTER &&
                         ops[0].reg.value == ZYDIS_REGISTER_RSP;
    u32 mem_count = 0;
    for (u32 i = 0; i < z.operand_count_visible; ++i) {
        const auto& op = ops[i];
        if (op.type != ZYDIS_OPERAND_TYPE_MEMORY ||
            op.visibility != ZYDIS_OPERAND_VISIBILITY_EXPLICIT || lea_rsp) {
            continue;
        }
        ++mem_count;
        const auto& mem = op.mem;
        if (mem.type == ZYDIS_MEMOP_TYPE_MIB) {
            s.modelled = false;
        }
        if (mem.base == ZYDIS_REGISTER_RSP) {
            s.mem_kind = MemKind::Sp;
        } else if (IsFamily(mem.base, ZYDIS_REGISTER_RSP) || mem.base == ZYDIS_REGISTER_EIP ||
                   IsFamily(mem.index, ZYDIS_REGISTER_RSP)) {
            s.mem_kind = MemKind::Odd;
        } else if (mem.base == ZYDIS_REGISTER_RBP) {
            s.mem_kind = MemKind::Bp;
        } else if (mem.base == ZYDIS_REGISTER_RIP) {
            s.mem_kind = MemKind::Rip;
        } else {
            s.mem_kind = MemKind::Other;
        }
        s.mem_indexed = mem.index != ZYDIS_REGISTER_NONE;
        s.mem_lea = mem.type == ZYDIS_MEMOP_TYPE_AGEN;
        s.mem_disp = mem.disp.size ? mem.disp.value : 0;
        s.mem_size = s.mem_lea ? 1 : static_cast<u16>(std::max<u32>(op.size / 8, 1));
    }
    if (mem_count > 1) {
        s.modelled = false;
    }
    if (z.attributes & (ZYDIS_ATTRIB_HAS_SEGMENT_FS | ZYDIS_ATTRIB_HAS_SEGMENT_GS)) {
        s.mem_kind = MemKind::None;
        s.keeps_address = true;
    }
    if (mnemonic == ZYDIS_MNEMONIC_EXTRQ || mnemonic == ZYDIS_MNEMONIC_INSERTQ ||
        mnemonic == ZYDIS_MNEMONIC_MOVNTSS || mnemonic == ZYDIS_MNEMONIC_MOVNTSD) {
        s.keeps_address = true;
    }
    const bool has_modrm = (z.attributes & ZYDIS_ATTRIB_HAS_MODRM) != 0;
    s.rip_relative = has_modrm && z.raw.modrm.mod == 0 && z.raw.modrm.rm == 5;
    if (s.rip_relative) {
        s.disp_offset = z.raw.disp.offset;
        s.modelled &= z.raw.disp.size == 32 && !(z.attributes & ZYDIS_ATTRIB_HAS_ADDRESSSIZE);
    } else if (s.mem_kind == MemKind::Sp) {
        s.reencodable = (z.encoding == ZYDIS_INSTRUCTION_ENCODING_LEGACY ||
                         z.encoding == ZYDIS_INSTRUCTION_ENCODING_VEX) &&
                        has_modrm && (z.attributes & ZYDIS_ATTRIB_HAS_SIB) &&
                        !(z.attributes & ZYDIS_ATTRIB_HAS_ADDRESSSIZE) && z.raw.modrm.mod != 3 &&
                        z.raw.modrm.rm == 4 && z.raw.sib.base == 4;
        s.modrm_offset = z.raw.modrm.offset;
        s.sib_offset = z.raw.sib.offset;
        s.disp_offset = z.raw.disp.offset;
        s.disp_size = static_cast<u8>(z.raw.disp.size / 8);
    }

    bool writes_rsp = false;
    bool writes_rbp = false;
    for (u32 i = 0; i < z.operand_count; ++i) {
        if (ops[i].type == ZYDIS_OPERAND_TYPE_REGISTER &&
            (ops[i].actions & ZYDIS_OPERAND_ACTION_MASK_WRITE)) {
            writes_rsp |= IsFamily(ops[i].reg.value, ZYDIS_REGISTER_RSP);
            writes_rbp |= IsFamily(ops[i].reg.value, ZYDIS_REGISTER_RBP);
        }
    }
    const bool reg0 = z.operand_count_visible > 0 && ops[0].type == ZYDIS_OPERAND_TYPE_REGISTER;
    switch (mnemonic) {
    case ZYDIS_MNEMONIC_RET:
        break;
    case ZYDIS_MNEMONIC_PUSH:
    case ZYDIS_MNEMONIC_PUSHFQ:
        s.push = true;
        s.sp_effect = z.operand_width == 64 ? SpEffect::Add : SpEffect::Unsupported;
        s.sp_delta = -8;
        s.rsp_value = reg0 && IsFamily(ops[0].reg.value, ZYDIS_REGISTER_RSP);
        s.modelled &= s.mem_kind != MemKind::Sp;
        break;
    case ZYDIS_MNEMONIC_POP:
    case ZYDIS_MNEMONIC_POPFQ:
        s.pop = true;
        s.sp_effect = z.operand_width == 64 ? SpEffect::Add : SpEffect::Unsupported;
        s.sp_delta = 8;
        if (reg0 && ops[0].reg.value == ZYDIS_REGISTER_RBP) {
            s.fp_effect = FpEffect::Pop;
        } else if (reg0 && IsFamily(ops[0].reg.value, ZYDIS_REGISTER_RSP)) {
            s.sp_effect = SpEffect::Unsupported;
        } else if (writes_rbp) {
            s.fp_effect = FpEffect::Kill;
        }
        s.modelled &= s.mem_kind != MemKind::Sp;
        break;
    case ZYDIS_MNEMONIC_LEAVE:
        s.sp_effect = SpEffect::Leave;
        s.fp_effect = FpEffect::Kill;
        break;
    default:
        if (writes_rsp) {
            const bool on_rsp = reg0 && ops[0].reg.value == ZYDIS_REGISTER_RSP;
            if (on_rsp && (mnemonic == ZYDIS_MNEMONIC_ADD || mnemonic == ZYDIS_MNEMONIC_SUB) &&
                ops[1].type == ZYDIS_OPERAND_TYPE_IMMEDIATE) {
                const s64 value = ops[1].imm.value.s;
                s.sp_effect = SpEffect::Add;
                s.sp_delta = static_cast<s32>(mnemonic == ZYDIS_MNEMONIC_ADD ? value : -value);
            } else if (lea_rsp && ops[1].mem.base == ZYDIS_REGISTER_RSP &&
                       ops[1].mem.index == ZYDIS_REGISTER_NONE) {
                s.sp_effect = SpEffect::Add;
                s.sp_delta =
                    static_cast<s32>(ops[1].mem.disp.size ? ops[1].mem.disp.value : 0);
            } else {
                s.sp_effect = SpEffect::Unsupported;
            }
        } else {
            for (u32 i = 0; i < z.operand_count_visible; ++i) {
                s.rsp_value |= ops[i].type == ZYDIS_OPERAND_TYPE_REGISTER &&
                               IsFamily(ops[i].reg.value, ZYDIS_REGISTER_RSP);
            }
        }
        if (writes_rbp) {
            const bool sets_frame = mnemonic == ZYDIS_MNEMONIC_MOV && reg0 &&
                                    ops[0].reg.value == ZYDIS_REGISTER_RBP &&
                                    ops[1].type == ZYDIS_OPERAND_TYPE_REGISTER &&
                                    ops[1].reg.value == ZYDIS_REGISTER_RSP;
            s.fp_effect = sets_frame ? FpEffect::Set : FpEffect::Kill;
        }
        break;
    }
    return s;
}

bool Overlap(const std::vector<Range>& a, const std::vector<Range>& b) {
    for (const auto& [lo, hi] : a) {
        for (const auto& [lo2, hi2] : b) {
            if (lo < hi2 && lo2 < hi) {
                return true;
            }
        }
    }
    return false;
}

/// Follows a clang-style table of offsets relative to itself that feeds the indirect jump at
/// insns[index], and queues its targets. False when no table is recognised.
bool QueueJumpTable(const Image& image, const Function& fn, Workspace& ws, u32 index, s32 sp,
                    s32 fp) {
    for (u32 back = 1; back < 12 && back <= index; ++back) {
        const Insn& p = ws.insns[index - back];
        if (p.mem_lea && p.mem_kind == MemKind::Rip) {
            const u64 table = fn.start + p.offset + p.length + p.mem_disp;
            bool any = false;
            for (u32 n = 0; n < 4096 && table + 4 * n + 4 <= image.size; ++n) {
                // The table is wherever the code says: read it in a way that cannot fault.
                s32 entry;
                SIZE_T read = 0;
                if (!ReadProcessMemory(GetCurrentProcess(), image.data + table + 4 * n, &entry,
                                       sizeof(entry), &read) ||
                    read != sizeof(entry)) {
                    break;
                }
                const u64 target = table + entry;
                if (target <= fn.start || target >= fn.start + fn.size ||
                    ws.index_of[target - fn.start] < 0) {
                    break;
                }
                ws.work.emplace_back(ws.index_of[target - fn.start], sp, fp);
                any = true;
            }
            return any;
        }
        if (p.flow == Flow::Jmp || p.flow == Flow::Ret) {
            break;
        }
    }
    return false;
}

/// Decodes and analyses one function. The verdict is None when it has calls (then RSP is below
/// its data whenever something else runs) or never touches memory below RSP.
Result Analyse(const Image& image, const Function& fn, Workspace& ws) {
    Result result{fn.start, fn.size, Verdict::None, false, 0, 0, nullptr};
    const u8* code = image.data + fn.start;
    const char* common = nullptr;
    const char* frame = nullptr;
    const char* move = nullptr;

    ws.insns.clear();
    ws.index_of.assign(fn.size, -1);
    bool below = false;
    u32 offset = 0;
    while (offset < fn.size) {
        ZydisDecodedInstruction z;
        ZydisDecodedOperand ops[ZYDIS_MAX_OPERAND_COUNT];
        if (!ZYAN_SUCCESS(ZydisDecoderDecodeFull(&ws.decoder, code + offset, fn.size - offset, &z,
                                                 ops))) {
            Note(common, "undecodable bytes");
            break;
        }
        if (z.meta.category == ZYDIS_CATEGORY_CALL) {
            return result;
        }
        Insn s = Summarize(z, ops, code + offset, fn.start + offset);
        s.offset = offset;
        below |= (s.mem_kind == MemKind::Sp || s.mem_kind == MemKind::Bp) && s.mem_disp < 0;
        ws.index_of[offset] = static_cast<s32>(ws.insns.size());
        ws.insns.push_back(s);
        offset += z.length;
    }
    if (!below || ws.insns.empty()) {
        return result;
    }
    if (fn.size < 5) {
        Note(common, "shorter than 5 bytes");
    }
    // push rbp; mov rbp,rsp in either encoding of the mov
    const bool framed =
        fn.size >= 4 && code[0] == 0x55 && code[1] == 0x48 &&
        ((code[2] == 0x89 && code[3] == 0xE5) || (code[2] == 0x8B && code[3] == 0xEC));

    ws.states.assign(ws.insns.size(), State{0, NoFrame, false});
    ws.work.clear();
    ws.frame_ranges.clear();
    ws.red_ranges.clear();
    ws.kept_ranges.clear();
    ws.pushed.clear();
    ws.popped.clear();
    ws.leave_offsets.clear();
    s32 lowest = 0;
    s32 lowest_indexed = 0;
    s32 sp_min = 0;
    const u64 start = fn.start;
    const u64 end = start + fn.size;
    const auto inside = [&](u64 target) { return target > start && target < end; };

    ws.work.emplace_back(0, 0, NoFrame);
    while (!ws.work.empty()) {
        auto [index, sp, fp] = ws.work.back();
        ws.work.pop_back();
        for (;;) {
            State& state = ws.states[index];
            if (state.seen) {
                if (state.sp != sp || state.fp != fp) {
                    Note(common, "inconsistent stack state");
                }
                break;
            }
            state = State{sp, fp, true};
            const Insn& s = ws.insns[index];
            if (!s.modelled) {
                Note(common, "instruction not modelled");
            }
            sp_min = std::min(sp_min, sp);
            if (framed && fp == NoFrame && s.offset > 1 && s.flow != Flow::Ret &&
                s.flow != Flow::Jmp) {
                Note(frame, "instruction after the frame is gone");
            }
            if (s.keeps_address) {
                Note(move, "instruction patched when it first runs");
            }
            const s32 disp = static_cast<s32>(s.mem_disp);
            if (s.mem_kind == MemKind::Odd) {
                Note(common, "32-bit stack pointer as a base");
            } else if (s.mem_kind == MemKind::Sp) {
                Note(frame, "rsp-relative access");
                if (!s.reencodable) {
                    Note(move, "encoding not rewritten");
                }
                const s32 lo = sp + disp;
                if (disp < 0) {
                    (s.mem_indexed ? lowest_indexed : lowest) =
                        std::min(s.mem_indexed ? lowest_indexed : lowest, disp);
                    ws.red_ranges.emplace_back(lo, lo + s.mem_size);
                    if (disp + s.mem_size > 0 && sp != 0) {
                        Note(move, "access straddles rsp");
                    }
                } else if (lo < 0) {
                    ws.kept_ranges.emplace_back(lo, lo + s.mem_size);
                    if (lo + s.mem_size > 0) {
                        Note(move, "access straddles the return address");
                    }
                }
                if (sp != 0 && (s.mem_indexed || s.mem_lea)) {
                    Note(move, "indexed or address-taken stack access with rsp moved");
                }
            } else if (s.mem_kind == MemKind::Bp && fp != NoFrame) {
                const s32 lo = fp + disp;
                if (!(lo == fp && s.mem_size == 8)) {
                    ws.frame_ranges.emplace_back(lo, lo + s.mem_size);
                }
                if (lo < sp) {
                    (s.mem_indexed ? lowest_indexed : lowest) =
                        std::min(s.mem_indexed ? lowest_indexed : lowest, lo - sp);
                    Note(move, "red zone reached through rbp");
                }
            }
            if (s.rsp_value) {
                if (s.fp_effect != FpEffect::Set) {
                    Note(frame, "rsp read as a value");
                }
                Note(move, "rsp read as a value");
            }

            bool lost = false;
            switch (s.sp_effect) {
            case SpEffect::None:
                break;
            case SpEffect::Add:
                if (s.push) {
                    ws.pushed.push_back(sp - 8);
                } else if (s.pop) {
                    ws.popped.push_back(sp);
                }
                sp += s.sp_delta;
                break;
            case SpEffect::Leave:
                Note(move, "leave");
                if (fp == NoFrame) {
                    Note(common, "rsp unknown");
                    lost = true;
                } else {
                    sp = fp + 8;
                }
                break;
            case SpEffect::Unsupported:
                Note(common, "rsp written in an unsupported way");
                lost = true;
                break;
            }
            if (lost) {
                break;
            }
            switch (s.fp_effect) {
            case FpEffect::None:
                break;
            case FpEffect::Set:
                if (s.offset != 1) {
                    Note(frame, "frame pointer set outside the prologue");
                }
                fp = sp;
                break;
            case FpEffect::Pop:
                if (fp != NoFrame) {
                    if (sp != fp + 8) {
                        Note(frame, "pop rbp with rsp away from the frame");
                    } else if (s.length != 1) {
                        Note(frame, "pop rbp in a long encoding");
                    } else {
                        ws.leave_offsets.push_back(s.offset);
                        if (index + 1 >= ws.insns.size() ||
                            (ws.insns[index + 1].flow != Flow::Ret &&
                             ws.insns[index + 1].flow != Flow::Jmp)) {
                            Note(frame, "pop rbp not followed by ret or jmp");
                        }
                    }
                }
                fp = NoFrame;
                break;
            case FpEffect::Kill:
                if (s.sp_effect != SpEffect::Leave && fp != NoFrame) {
                    Note(frame, "rbp written");
                }
                fp = NoFrame;
                break;
            }

            bool done = false;
            u64 next = start + s.offset + s.length;
            switch (s.flow) {
            case Flow::Ret:
                if (sp != 0) {
                    Note(common, "ret with rsp away from the return address");
                }
                if (fp != NoFrame) {
                    Note(frame, "ret with a live frame");
                }
                done = true;
                break;
            case Flow::Stop:
                done = true;
                break;
            case Flow::Jcc:
                if (s.form == JumpForm::ShortOnly) {
                    Note(move, "jrcxz or loop");
                } else if (!s.plain_branch) {
                    Note(move, "branch with prefixes");
                }
                if (inside(s.target)) {
                    if (s.target < start + 4) {
                        Note(frame, "branch into the prologue");
                    }
                    if (ws.index_of[s.target - start] < 0) {
                        Note(common, "flow leaves the decoded instructions");
                    } else {
                        ws.work.emplace_back(ws.index_of[s.target - start], sp, fp);
                    }
                } else {
                    if (sp != 0) {
                        Note(common, "jump out with rsp away from the return address");
                    }
                    if (fp != NoFrame) {
                        Note(frame, "jump out with a live frame");
                    }
                }
                break;
            case Flow::Jmp:
                done = true;
                if (s.form != JumpForm::Direct) {
                    const bool exits = sp == 0 && fp == NoFrame;
                    if (s.form == JumpForm::Memory && exits) {
                        break;
                    }
                    if (s.form == JumpForm::Register && exits && framed) {
                        break;
                    }
                    Note(move, "indirect jump through a register or a table");
                    if (!QueueJumpTable(image, fn, ws, index, sp, fp)) {
                        Note(common, "unresolved indirect jump");
                    }
                    break;
                }
                if (!s.plain_branch) {
                    Note(move, "branch with prefixes");
                }
                if (inside(s.target)) {
                    if (s.target < start + 4) {
                        Note(frame, "branch into the prologue");
                    }
                    next = s.target;
                    done = false;
                    break;
                }
                if (sp != 0) {
                    Note(common, "jump out with rsp away from the return address");
                }
                if (fp != NoFrame) {
                    Note(frame, "jump out with a live frame");
                }
                break;
            case Flow::None:
                break;
            }
            if (done) {
                break;
            }
            if (next >= end) {
                Note(common, "flow runs past the end");
                break;
            }
            const s32 next_index = ws.index_of[next - start];
            if (next_index < 0) {
                Note(common, "flow leaves the decoded instructions");
                break;
            }
            index = static_cast<u32>(next_index);
        }
    }

    for (size_t i = 0; i < ws.insns.size(); ++i) {
        if (!ws.states[i].seen && !ws.insns[i].padding) {
            Note(common, "unreached code");
            break;
        }
    }
    for (const s32 slot : ws.popped) {
        if (std::find(ws.pushed.begin(), ws.pushed.end(), slot) == ws.pushed.end()) {
            Note(common, "pop of a slot that was not pushed");
            break;
        }
    }
    std::vector<Range> slots;
    for (const s32 slot : ws.pushed) {
        // The saved rbp of the prologue stays where it was: "enter" pushes it too.
        if (slot != -8 || !framed) {
            slots.emplace_back(slot, slot + 8);
        }
    }
    if (Overlap(ws.frame_ranges, slots)) {
        Note(frame, "frame access overlaps a pushed slot");
    }
    slots.clear();
    for (const s32 slot : ws.pushed) {
        slots.emplace_back(slot, slot + 8);
    }
    if (Overlap(ws.red_ranges, ws.kept_ranges) || Overlap(ws.red_ranges, slots)) {
        Note(move, "red zone overlaps the allocated stack");
    }
    if (std::min(lowest, lowest_indexed) < -RedZoneSize) {
        Note(common, "deeper than the red zone");
    }
    // How far RSP is lowered. Everything the function may own lies between 128 bytes below its
    // lowest RSP and the return address; a copy runs with RSP below all of that. The frame
    // reserved in place starts below the saved rbp, so it is 8 bytes shorter; the registers
    // pushed after it then land below everything the function reaches through rbp.
    const s32 move_shift = (RedZoneSize - sp_min + 15) & ~15;
    const s32 frame_shift = (RedZoneSize - 8 - sp_min + 15) & ~15;
    if (move_shift > MaxShift) {
        Note(move, "stack too deep");
        Note(frame, "stack too deep");
    }
    for (const Range& range : ws.frame_ranges) {
        if (range.first < -8 - frame_shift) {
            Note(frame, "frame access below the reserved stack");
        }
    }

    result.lowest = std::min(lowest, lowest_indexed);
    result.indexed_only = lowest == 0;
    if (result.lowest == 0) {
        return result;
    }
    if (framed) {
        result.reason = common ? common : frame;
        result.verdict = result.reason ? Verdict::Left : Verdict::Frame;
        result.shift = frame_shift;
    } else {
        result.reason = common ? common : move;
        result.verdict = result.reason ? Verdict::Left : Verdict::Move;
        result.shift = move_shift;
    }
    return result;
}

/// Marks the candidates that a direct branch or call elsewhere in the module enters past their
/// first instruction: such code would run with the stack laid out the old way.
void FindOutsideEntries(const Image& image, const std::vector<Function>& functions,
                        const std::vector<Result>& results,
                        std::vector<std::atomic<bool>>& entered) {
    std::atomic<size_t> next{0};
    constexpr size_t Chunk = 512;
    const auto worker = [&] {
        ZydisDecoder decoder;
        ZydisDecoderInit(&decoder, ZYDIS_MACHINE_MODE_LONG_64, ZYDIS_STACK_WIDTH_64);
        for (;;) {
            const size_t first = next.fetch_add(Chunk);
            if (first >= functions.size()) {
                break;
            }
            const size_t last = std::min(first + Chunk, functions.size());
            for (size_t i = first; i < last; ++i) {
                const Function& fn = functions[i];
                u32 offset = 0;
                while (offset < fn.size) {
                    ZydisDecodedInstruction z;
                    if (!ZYAN_SUCCESS(ZydisDecoderDecodeInstruction(
                            &decoder, nullptr, image.data + fn.start + offset, fn.size - offset,
                            &z))) {
                        break;
                    }
                    offset += z.length;
                    if (!z.raw.imm[0].is_relative) {
                        continue;
                    }
                    const u64 target = fn.start + offset + z.raw.imm[0].value.s;
                    if (target >= fn.start && target < fn.start + fn.size) {
                        continue;
                    }
                    auto it = std::upper_bound(
                        results.begin(), results.end(), target,
                        [](u64 value, const Result& result) { return value < result.start; });
                    if (it != results.begin() && target > (it - 1)->start &&
                        target < (it - 1)->start + (it - 1)->size) {
                        entered[it - 1 - results.begin()] = true;
                    }
                }
            }
        }
    };
    const u32 threads = std::clamp<u32>(std::thread::hardware_concurrency(), 1, 16);
    std::vector<std::thread> pool;
    for (u32 i = 1; i < threads && functions.size() > Chunk; ++i) {
        pool.emplace_back(worker);
    }
    worker();
    for (auto& thread : pool) {
        thread.join();
    }
}

void Put32(std::vector<u8>& out, s64 value) {
    const s32 v = static_cast<s32>(value);
    const u8* p = reinterpret_cast<const u8*>(&v);
    out.insert(out.end(), p, p + 4);
}

void PutLeaRsp(std::vector<u8>& out, s32 disp) {
    out.insert(out.end(), {0x48, 0x8D, 0xA4, 0x24});
    Put32(out, disp);
}

/// The displacement an RSP-relative operand needs once RSP runs `shift` lower: what lies below
/// RSP or at and above the return address keeps its address, what the function itself pushed or
/// reserved moves down with RSP.
s32 MovedDisp(const Insn& s, s32 sp, s32 shift) {
    const s32 disp = static_cast<s32>(s.mem_disp);
    return disp < 0 || sp + disp >= 0 ? disp + shift : disp;
}

u32 DispSize(s32 disp) {
    return disp == 0 ? 0 : (disp >= -128 && disp <= 127 ? 1 : 4);
}

/// Builds the copy of a function analysed as Verdict::Move, to run at `dest` (absolute). With
/// dest == 0 only the size is meaningful. `new_offsets` receives where each instruction went.
bool BuildCopy(const Image& image, u64 base, const Function& fn, const Workspace& ws, s32 shift,
               u64 dest, std::vector<u8>& out, std::vector<u32>& new_offsets) {
    const u8* code = image.data + fn.start;
    const u64 start = fn.start;
    const u64 end = start + fn.size;
    const auto inside = [&](u64 target) { return target > start && target < end; };
    const size_t count = ws.insns.size();

    new_offsets.assign(count + 1, 0);
    u32 at = LeaRspSize;
    u32 stubs = 0;
    for (size_t i = 0; i < count; ++i) {
        const Insn& s = ws.insns[i];
        new_offsets[i] = at;
        u32 length = s.length;
        if (s.flow == Flow::Ret) {
            length += LeaRspSize;
        } else if (s.flow == Flow::Jmp) {
            length = s.form == JumpForm::Direct ? (inside(s.target) ? 5 : LeaRspSize + 5)
                                                : LeaRspSize + s.length;
        } else if (s.flow == Flow::Jcc) {
            length = 6;
            stubs += inside(s.target) ? 0 : 1;
        } else if (s.mem_kind == MemKind::Sp) {
            length = length - s.disp_size + DispSize(MovedDisp(s, ws.states[i].sp, shift));
            if (length > 15) {
                return false;
            }
        }
        at += length;
    }
    new_offsets[count] = at;
    u32 stub_at = at;
    const u32 total = at + stubs * (LeaRspSize + 5);

    out.clear();
    out.reserve(total);
    PutLeaRsp(out, -shift);
    const auto rel_to = [&](u64 target_absolute, u32 next_offset) {
        return static_cast<s64>(target_absolute) - static_cast<s64>(dest + next_offset);
    };
    const auto copy_fixed = [&](const Insn& s, u32 new_offset) {
        const size_t first = out.size();
        out.insert(out.end(), code + s.offset, code + s.offset + s.length);
        if (s.rip_relative) {
            s32 disp;
            std::memcpy(&disp, &out[first + s.disp_offset], 4);
            const s64 moved = disp + static_cast<s64>(base + start + s.offset + s.length) -
                              static_cast<s64>(dest + new_offset + s.length);
            if (dest != 0 && moved != static_cast<s32>(moved)) {
                return false;
            }
            const s32 value = static_cast<s32>(moved);
            std::memcpy(&out[first + s.disp_offset], &value, 4);
        }
        return true;
    };
    for (size_t i = 0; i < count; ++i) {
        const Insn& s = ws.insns[i];
        const u32 here = new_offsets[i];
        if (s.flow == Flow::Ret) {
            PutLeaRsp(out, shift);
            out.insert(out.end(), code + s.offset, code + s.offset + s.length);
        } else if (s.flow == Flow::Jmp && s.form == JumpForm::Direct) {
            if (inside(s.target)) {
                out.push_back(0xE9);
                Put32(out, static_cast<s64>(new_offsets[ws.index_of[s.target - start]]) -
                               (here + 5));
            } else {
                PutLeaRsp(out, shift);
                out.push_back(0xE9);
                Put32(out, rel_to(base + s.target, here + LeaRspSize + 5));
            }
        } else if (s.flow == Flow::Jmp) {
            PutLeaRsp(out, shift);
            if (!copy_fixed(s, here + LeaRspSize)) {
                return false;
            }
        } else if (s.flow == Flow::Jcc) {
            const u8 condition = code[s.offset] == 0x0F ? code[s.offset + 1] & 0x0F
                                                         : code[s.offset] & 0x0F;
            out.push_back(0x0F);
            out.push_back(0x80 | condition);
            if (inside(s.target)) {
                Put32(out, static_cast<s64>(new_offsets[ws.index_of[s.target - start]]) -
                               (here + 6));
            } else {
                Put32(out, static_cast<s64>(stub_at) - (here + 6));
                stub_at += LeaRspSize + 5;
            }
        } else if (s.mem_kind == MemKind::Sp) {
            const s32 disp = MovedDisp(s, ws.states[i].sp, shift);
            const u32 size = DispSize(disp);
            const u8* bytes = code + s.offset;
            const size_t first = out.size();
            out.insert(out.end(), bytes, bytes + s.sib_offset + 1);
            out[first + s.modrm_offset] = static_cast<u8>(
                (bytes[s.modrm_offset] & 0x3F) | (size == 0 ? 0x00 : size == 1 ? 0x40 : 0x80));
            if (size == 1) {
                out.push_back(static_cast<u8>(disp));
            } else if (size == 4) {
                Put32(out, disp);
            }
            out.insert(out.end(), bytes + s.sib_offset + 1 + s.disp_size, bytes + s.length);
        } else if (!copy_fixed(s, here)) {
            return false;
        }
        if (out.size() != new_offsets[i + 1]) {
            return false;
        }
    }
    for (size_t i = 0; i < count; ++i) {
        const Insn& s = ws.insns[i];
        if (s.flow == Flow::Jcc && !inside(s.target)) {
            PutLeaRsp(out, shift);
            out.push_back(0xE9);
            Put32(out, rel_to(base + s.target, static_cast<u32>(out.size()) + 4));
        }
    }
    return out.size() == total;
}

class EhReader {
public:
    explicit EhReader(const Image& image_) : image(image_) {}

    bool Encoded(u64& pos, u8 encoding, u64 data_base, s64& value) {
        const u8 format = encoding & 0x0F;
        u32 size;
        switch (format) {
        case 0x00:
        case 0x04:
        case 0x0C:
            size = 8;
            break;
        case 0x03:
        case 0x0B:
            size = 4;
            break;
        case 0x02:
        case 0x0A:
            size = 2;
            break;
        default:
            return false;
        }
        if (pos + size > image.size) {
            return false;
        }
        const u8* p = image.data + pos;
        switch (format) {
        case 0x03:
            value = Read<u32>(p);
            break;
        case 0x0B:
            value = Read<s32>(p);
            break;
        case 0x02:
            value = Read<u16>(p);
            break;
        case 0x0A:
            value = Read<s16>(p);
            break;
        default:
            value = Read<s64>(p);
            break;
        }
        switch (encoding & 0x70) {
        case 0x00:
            break;
        case 0x10:
            value += pos;
            break;
        case 0x30:
            value += data_base;
            break;
        default:
            return false;
        }
        pos += size;
        return true;
    }

    bool Uleb(u64& pos, u64& value) {
        value = 0;
        for (u32 shift = 0; shift < 64; shift += 7) {
            if (pos >= image.size) {
                return false;
            }
            const u8 byte = image.data[pos++];
            value |= static_cast<u64>(byte & 0x7F) << shift;
            if (!(byte & 0x80)) {
                return true;
            }
        }
        return false;
    }

    template <typename T>
    static T Read(const u8* p) {
        T value;
        std::memcpy(&value, p, sizeof(T));
        return value;
    }

    /// The pointer encoding of the FDEs that use the CIE at `cie`, or -1.
    int CiePointerEncoding(u64 cie) {
        if (cie + 12 > image.size) {
            return -1;
        }
        const u8 version = image.data[cie + 8];
        u64 pos = cie + 9;
        std::string augmentation;
        while (pos < image.size && image.data[pos] != 0) {
            augmentation.push_back(static_cast<char>(image.data[pos++]));
        }
        ++pos;
        u64 ignored;
        if (!Uleb(pos, ignored) || !Uleb(pos, ignored)) {
            return -1;
        }
        if (version == 1) {
            ++pos;
        } else if (!Uleb(pos, ignored)) {
            return -1;
        }
        int encoding = 0;
        if (!augmentation.empty() && augmentation[0] == 'z') {
            if (!Uleb(pos, ignored)) {
                return -1;
            }
            for (size_t i = 1; i < augmentation.size() && pos < image.size; ++i) {
                switch (augmentation[i]) {
                case 'R':
                    encoding = image.data[pos++];
                    break;
                case 'L':
                    ++pos;
                    break;
                case 'P': {
                    const u8 personality = image.data[pos++];
                    s64 unused;
                    if (!Encoded(pos, personality, 0, unused)) {
                        return -1;
                    }
                    break;
                }
                default:
                    break;
                }
            }
        }
        return encoding;
    }

private:
    Image image;
};

/// One entry per FDE: the functions of the module, sorted, none overlapping another.
std::vector<Function> ReadFunctions(const Image& image, u64 hdr, u64 hdr_size) {
    std::vector<Function> functions;
    if (hdr == 0 || hdr_size < 12 || hdr + hdr_size > image.size || image.data[hdr] != 1) {
        return functions;
    }
    EhReader reader{image};
    const u8 pointer_encoding = image.data[hdr + 1];
    const u8 count_encoding = image.data[hdr + 2];
    const u8 table_encoding = image.data[hdr + 3];
    u64 pos = hdr + 4;
    s64 unused;
    s64 count;
    if (!reader.Encoded(pos, pointer_encoding, hdr, unused) ||
        !reader.Encoded(pos, count_encoding, hdr, count) || count <= 0) {
        return functions;
    }
    u64 known_cie = 0;
    int known_encoding = -1;
    functions.reserve(static_cast<size_t>(count));
    for (s64 i = 0; i < count; ++i) {
        s64 location;
        s64 fde_signed;
        if (!reader.Encoded(pos, table_encoding, hdr, location) ||
            !reader.Encoded(pos, table_encoding, hdr, fde_signed)) {
            break;
        }
        const u64 fde = static_cast<u64>(fde_signed);
        if (fde + 8 > image.size) {
            continue;
        }
        const u32 length = EhReader::Read<u32>(image.data + fde);
        if (length == 0 || length == 0xFFFFFFFF) {
            continue;
        }
        const u64 cie = fde + 4 - EhReader::Read<u32>(image.data + fde + 4);
        if (cie != known_cie || known_encoding < 0) {
            known_cie = cie;
            known_encoding = reader.CiePointerEncoding(cie);
        }
        if (known_encoding < 0) {
            continue;
        }
        u64 at = fde + 8;
        s64 begin;
        s64 size;
        if (!reader.Encoded(at, static_cast<u8>(known_encoding), 0, begin) ||
            !reader.Encoded(at, static_cast<u8>(known_encoding & 0x0F), 0, size)) {
            continue;
        }
        if (begin <= 0 || size <= 0 || static_cast<u64>(begin + size) > image.size ||
            size > std::numeric_limits<s32>::max()) {
            continue;
        }
        functions.push_back({static_cast<u32>(begin), static_cast<u32>(size)});
    }
    std::sort(functions.begin(), functions.end(), [](const Function& a, const Function& b) {
        return a.start != b.start ? a.start < b.start : a.size > b.size;
    });
    functions.erase(std::unique(functions.begin(), functions.end(),
                                [](const Function& a, const Function& b) {
                                    return a.start == b.start;
                                }),
                    functions.end());
    // Entries that overlap cannot both be functions: neither is trusted.
    std::vector<bool> bad(functions.size(), false);
    for (size_t i = 0; i + 1 < functions.size(); ++i) {
        if (functions[i].start + functions[i].size > functions[i + 1].start) {
            bad[i] = bad[i + 1] = true;
        }
    }
    size_t kept = 0;
    for (size_t i = 0; i < functions.size(); ++i) {
        if (!bad[i]) {
            functions[kept++] = functions[i];
        }
    }
    functions.resize(kept);
    return functions;
}

struct Entry {
    u64 start;
    u32 size;
    Verdict verdict;
    bool rewritten;
    s32 lowest;
    s32 shift;
    const char* reason;
    u64 copy;
    u32 copy_size;
    std::vector<u32> new_offsets;
    std::vector<u32> old_offsets;
};

struct ModuleEntries {
    std::string name;
    u64 base;
    u64 size;
    std::vector<Entry> entries;
    std::vector<u32> by_copy;
    std::unique_ptr<std::atomic<u32>[]> faults;
    // Statistics only: every function of the module and the exceptions each one took.
    std::vector<Function> functions;
    std::unique_ptr<std::atomic<u32>[]> function_faults;
};

struct Config {
    bool rewrite;
    bool stats;
    bool keep_maps;
    std::string report;
    std::string dump;
    std::string probe;
};

const Config& GetConfig() {
    static const Config config = [] {
        const auto text = [](const char* name) {
            const char* value = std::getenv(name);
            return std::string{value ? value : ""};
        };
        Config c;
        c.rewrite = text("SHADPS4_RED_ZONE") != "0";
        const std::string stats = text("SHADPS4_RED_ZONE_STATS");
        c.stats = !stats.empty() && stats != "0";
        c.report = text("SHADPS4_RED_ZONE_REPORT");
        c.dump = text("SHADPS4_RED_ZONE_DUMP");
        c.probe = text("SHADPS4_RED_ZONE_PROBE");
        c.keep_maps = !c.probe.empty() || !text("SHADPS4_GUEST_TRACE").empty();
        return c;
    }();
    return config;
}

std::shared_mutex registry_mutex;
std::deque<ModuleEntries> registry;

struct Found {
    ModuleEntries* module;
    Entry* entry;
    u32 index;
    bool in_copy;
};

/// The red-zone function (or its copy) that contains the address. Call with the registry locked.
Found Find(u64 address) {
    for (auto& module : registry) {
        if (address >= module.base && address < module.base + module.size) {
            auto it = std::upper_bound(
                module.entries.begin(), module.entries.end(), address,
                [](u64 value, const Entry& entry) { return value < entry.start; });
            if (it != module.entries.begin() && address < (it - 1)->start + (it - 1)->size) {
                --it;
                return {&module, &*it, static_cast<u32>(it - module.entries.begin()), false};
            }
            return {&module, nullptr, 0, false};
        }
        if (address >= module.base + module.size &&
            address < module.base + module.size + TrampolineSpan) {
            auto it = std::upper_bound(module.by_copy.begin(), module.by_copy.end(), address,
                                       [&](u64 value, u32 index) {
                                           return value < module.entries[index].copy;
                                       });
            if (it != module.by_copy.begin()) {
                Entry& entry = module.entries[*(it - 1)];
                if (address < entry.copy + entry.copy_size) {
                    return {&module, &entry, *(it - 1), true};
                }
            }
            return {&module, nullptr, 0, false};
        }
    }
    return {nullptr, nullptr, 0, false};
}

const char* VerdictName(const Entry& entry) {
    if (entry.rewritten) {
        return entry.verdict == Verdict::Frame ? "rewritten in place" : "moved";
    }
    return entry.verdict == Verdict::Left ? "left alone" : "not rewritten";
}

std::atomic<u64> faults_total{0};
std::atomic<u64> faults_guest{0};
std::atomic<u64> faults_exposed{0};
std::atomic<u64> faults_protected{0};
std::atomic<u64> faults_unknown_code{0};
std::atomic<s64> last_dump{0};

void DumpStats() {
    LOG_INFO(Core,
             "RedZone stats: {} handled exceptions, {} in guest code, {} inside functions with "
             "data below RSP ({} protected by the rewrite, {} exposed)",
             faults_total.load(), faults_guest.load(),
             faults_exposed.load() + faults_protected.load(), faults_protected.load(),
             faults_exposed.load());
    LOG_INFO(Core, "RedZone stats: {} in guest code that no unwind entry covers",
             faults_unknown_code.load());
    for (const auto& module : registry) {
        std::vector<std::pair<u32, u32>> busiest;
        for (u32 i = 0; i < module.functions.size(); ++i) {
            if (const u32 count = module.function_faults[i].load(); count != 0) {
                busiest.emplace_back(count, i);
            }
        }
        std::sort(busiest.rbegin(), busiest.rend());
        LOG_INFO(Core, "RedZone stats: {}: {} functions took exceptions", module.name,
                 busiest.size());
        for (size_t i = 0; i < busiest.size() && i < 12; ++i) {
            const Function& fn = module.functions[busiest[i].second];
            LOG_INFO(Core, "RedZone stats: {} function {:#x} size {}: {} exceptions", module.name,
                     fn.start, fn.size, busiest[i].first);
        }
    }
    for (const auto& module : registry) {
        for (size_t i = 0; i < module.entries.size(); ++i) {
            const u32 count = module.faults[i].load();
            if (count != 0) {
                const Entry& entry = module.entries[i];
                LOG_INFO(Core, "RedZone stats: {} {:#x} size {} lowest {} {}{}{}: {} exceptions",
                         module.name, entry.start - module.base, entry.size, entry.lowest,
                         VerdictName(entry), entry.reason ? ", " : "",
                         entry.reason ? entry.reason : "", count);
            }
        }
    }
}

struct Probe {
    u64 vaddr;
    u8* breakpoint;
    std::atomic<u64> hits;
    u64 changed;
};

constexpr u32 ProbeRingSlots = 256;
alignas(16) u8 probe_ring[ProbeRingSlots][256];
u64 probe_ring_next = 0;
std::deque<Probe> probes;

/// Replaces the instructions at a probe address with a jump to code that saves the function's
/// data below RSP, raises a breakpoint the exception handler resumes, compares, and goes on.
/// Each use takes a slot of probe_ring: the data at 0, five saved registers at 0x80, ymm15 at 0xC0.
void InstallProbe(u64 base, u64 vaddr) {
    Entry* entry;
    u64 site;
    {
        std::shared_lock lock{registry_mutex};
        const Found found = Find(base + vaddr);
        entry = found.entry;
        if (entry == nullptr || found.in_copy) {
            LOG_ERROR(Core, "RedZone probe {:#x}: not inside a function with data below RSP",
                      vaddr);
            return;
        }
    }
    site = CurrentAddress(base + vaddr);
    if (entry->verdict == Verdict::Move && entry->rewritten && site == base + vaddr) {
        LOG_ERROR(Core, "RedZone probe {:#x}: not at an instruction of the moved function", vaddr);
        return;
    }
    ZydisDecoder decoder;
    ZydisDecoderInit(&decoder, ZYDIS_MACHINE_MODE_LONG_64, ZYDIS_STACK_WIDTH_64);
    u32 covered = 0;
    while (covered < 5) {
        ZydisDecodedInstruction z;
        ZydisDecodedOperand ops[ZYDIS_MAX_OPERAND_COUNT];
        const u8* at = reinterpret_cast<const u8*>(site) + covered;
        if (!ZYAN_SUCCESS(ZydisDecoderDecodeFull(&decoder, at, 15, &z, ops))) {
            LOG_ERROR(Core, "RedZone probe {:#x}: undecodable", vaddr);
            return;
        }
        const Insn s = Summarize(z, ops, at, 0);
        if (s.flow != Flow::None || s.rip_relative || !s.modelled ||
            s.sp_effect != SpEffect::None || s.fp_effect != FpEffect::None) {
            LOG_ERROR(Core, "RedZone probe {:#x}: needs 5 bytes of plain instructions", vaddr);
            return;
        }
        covered += z.length;
    }
    constexpr u32 StubSize = 512;
    u8* stub = AllocateTrampoline(reinterpret_cast<void*>(base), StubSize);
    if (stub == nullptr) {
        LOG_ERROR(Core, "RedZone probe {:#x}: no room for the stub", vaddr);
        return;
    }
    Probe& probe = probes.emplace_back();
    probe.vaddr = vaddr;
    const s32 shift = entry->rewritten ? entry->shift : 0;
    const s32 low = shift + entry->lowest;
    const u32 length = static_cast<u32>(-entry->lowest);

    using namespace Xbyak::util;
    Xbyak::CodeGenerator c(StubSize, stub);
    Xbyak::Label same;
    c.lea(rsp, ptr[rsp - 0x100]);
    c.push(rax);
    c.push(rcx);
    c.push(rsi);
    c.push(rdi);
    c.pushfq();
    c.mov(eax, 1);
    c.mov(rcx, reinterpret_cast<u64>(&probe_ring_next));
    c.lock();
    c.xadd(qword[rcx], rax);
    c.and_(eax, ProbeRingSlots - 1);
    c.shl(rax, 8);
    c.mov(rcx, reinterpret_cast<u64>(&probe_ring[0][0]));
    c.add(rax, rcx);
    for (u32 i = 0; i < 5; ++i) {
        c.mov(rcx, ptr[rsp + 8 * i]);
        c.mov(ptr[rax + 0x80 + 8 * i], rcx);
    }
    c.lea(rsp, ptr[rsp + 0x128]);
    c.lea(rsi, ptr[rsp + low]);
    c.mov(rdi, rax);
    c.mov(ecx, length);
    c.rep();
    c.movsb();
    c.mov(rsi, rax);
    // Windows saves the upper halves of the YMM registers right below RSP, and only when the
    // thread has them in use: here they are, as in the worst case.
    c.vmovdqu(ptr[rax + 0xC0], ymm15);
    c.vcmpps(ymm15, ymm15, ymm15, 0x0F);
    probe.breakpoint = const_cast<u8*>(c.getCurr());
    c.int3();
    c.mov(rax, rsi);
    c.vmovdqu(ymm15, ptr[rax + 0xC0]);
    c.lea(rdi, ptr[rsp + low]);
    c.mov(ecx, length);
    c.repe();
    c.cmpsb();
    c.je(same);
    c.mov(rcx, reinterpret_cast<u64>(&probe.changed));
    c.lock();
    c.inc(qword[rcx]);
    // Counted; the data is put back so that the run can go on and be counted again.
    c.mov(rsi, rax);
    c.lea(rdi, ptr[rsp + low]);
    c.mov(ecx, length);
    c.rep();
    c.movsb();
    c.L(same);
    c.lea(rsp, ptr[rsp - 0x100]);
    c.push(qword[rax + 0x80]);
    c.popfq();
    c.lea(rsp, ptr[rsp + 0x100]);
    c.mov(rdi, ptr[rax + 0x88]);
    c.mov(rsi, ptr[rax + 0x90]);
    c.mov(rcx, ptr[rax + 0x98]);
    c.mov(rax, ptr[rax + 0xA0]);
    c.db(reinterpret_cast<const u8*>(site), covered);
    c.jmp(reinterpret_cast<const void*>(site + covered), Xbyak::CodeGenerator::T_NEAR);

    u8* patch = reinterpret_cast<u8*>(site);
    const s32 rel = static_cast<s32>(reinterpret_cast<s64>(stub) - static_cast<s64>(site + 5));
    patch[0] = 0xE9;
    std::memcpy(patch + 1, &rel, 4);
    std::memset(patch + 5, 0x90, covered - 5);
    LOG_CRITICAL(Core,
                 "RedZone probe {:#x}: function {:#x} ({}), watching {} bytes at rsp{:+}, "
                 "instructions now at {:#x}",
                 vaddr, entry->start - base, VerdictName(*entry), length, low, site);
}

void InstallProbes(const Image& image, u64 base) {
    const std::string& spec = GetConfig().probe;
    size_t start = 0;
    while (start < spec.size()) {
        const size_t end = std::min(spec.find(',', start), spec.size());
        const u64 vaddr = std::strtoull(spec.substr(start, end - start).c_str(), nullptr, 16);
        start = end + 1;
        if (vaddr != 0 && vaddr < image.size) {
            InstallProbe(base, vaddr);
        }
    }
}

} // namespace

void OnModuleLoaded(std::string_view name, u64 base, u64 size, u64 eh_frame_hdr,
                    u64 eh_frame_hdr_size) {
    const Config& config = GetConfig();
    if (!config.rewrite && !config.stats && config.report.empty() && config.probe.empty()) {
        return;
    }
    const auto started = std::chrono::steady_clock::now();
    const Image image{reinterpret_cast<const u8*>(base), size};
    const std::vector<Function> functions = ReadFunctions(image, eh_frame_hdr, eh_frame_hdr_size);

    std::vector<Result> results;
    {
        std::atomic<size_t> next{0};
        std::mutex results_mutex;
        constexpr size_t Chunk = 512;
        const auto worker = [&] {
            Workspace ws;
            std::vector<Result> found;
            for (;;) {
                const size_t first = next.fetch_add(Chunk);
                if (first >= functions.size()) {
                    break;
                }
                const size_t last = std::min(first + Chunk, functions.size());
                for (size_t i = first; i < last; ++i) {
                    const Result result = Analyse(image, functions[i], ws);
                    if (result.verdict != Verdict::None) {
                        found.push_back(result);
                    }
                }
            }
            std::scoped_lock lock{results_mutex};
            results.insert(results.end(), found.begin(), found.end());
        };
        const u32 threads = std::clamp<u32>(std::thread::hardware_concurrency(), 1, 16);
        std::vector<std::thread> pool;
        for (u32 i = 1; i < threads && functions.size() > Chunk; ++i) {
            pool.emplace_back(worker);
        }
        worker();
        for (auto& thread : pool) {
            thread.join();
        }
    }
    std::sort(results.begin(), results.end(),
              [](const Result& a, const Result& b) { return a.start < b.start; });
    if (config.rewrite || !config.report.empty()) {
        std::vector<std::atomic<bool>> entered(results.size());
        FindOutsideEntries(image, functions, results, entered);
        for (size_t i = 0; i < results.size(); ++i) {
            if (entered[i] && results[i].verdict != Verdict::Left) {
                results[i].verdict = Verdict::Left;
                results[i].reason = "entered past its first instruction from another function";
            }
        }
    }

    ModuleEntries module;
    module.name = name;
    module.base = base;
    module.size = size;
    u32 in_place = 0;
    u32 moved = 0;
    u32 left = 0;
    Workspace ws;
    std::vector<u8> copy;
    std::vector<u32> new_offsets;
    for (const Result& result : results) {
        Entry entry{};
        entry.start = base + result.start;
        entry.size = result.size;
        entry.verdict = result.verdict;
        entry.lowest = result.lowest;
        entry.shift = result.shift;
        entry.reason = result.reason;
        u8* code = reinterpret_cast<u8*>(entry.start);
        const Function fn{result.start, result.size};
        if (result.verdict == Verdict::Left) {
            ++left;
            LOG_INFO(Core, "RedZone: {} {:#x} (size {}, down to rsp{}) left alone: {}", name,
                     result.start, result.size, result.lowest, result.reason);
        } else if (!config.rewrite) {
            // Analysed for the statistics or the report only.
        } else if (result.verdict == Verdict::Frame) {
            // The offsets of "pop rbp" are found again: the worker's own list is not kept.
            if (Analyse(image, fn, ws).verdict == Verdict::Frame) {
                const u8 enter[4] = {0xC8, static_cast<u8>(result.shift),
                                     static_cast<u8>(result.shift >> 8), 0x00};
                std::memcpy(code, enter, sizeof(enter));
                for (const u32 offset : ws.leave_offsets) {
                    code[offset] = 0xC9;
                }
                entry.rewritten = true;
                ++in_place;
            }
        } else if (Analyse(image, fn, ws).verdict == Verdict::Move &&
                   BuildCopy(image, base, fn, ws, result.shift, 0, copy, new_offsets)) {
            u8* dest = AllocateTrampoline(reinterpret_cast<void*>(base), copy.size());
            if (dest != nullptr && BuildCopy(image, base, fn, ws, result.shift,
                                             reinterpret_cast<u64>(dest), copy, new_offsets)) {
                std::memcpy(dest, copy.data(), copy.size());
                const s64 rel = reinterpret_cast<s64>(dest) - static_cast<s64>(entry.start + 5);
                const s32 rel32 = static_cast<s32>(rel);
                code[0] = 0xE9;
                std::memcpy(code + 1, &rel32, 4);
                entry.rewritten = true;
                entry.copy = reinterpret_cast<u64>(dest);
                entry.copy_size = static_cast<u32>(copy.size());
                if (config.keep_maps) {
                    entry.new_offsets.assign(new_offsets.begin(), new_offsets.end() - 1);
                    for (const Insn& s : ws.insns) {
                        entry.old_offsets.push_back(s.offset);
                    }
                }
                ++moved;
            }
        }
        if (config.rewrite && result.verdict != Verdict::Left && !entry.rewritten) {
            ++left;
            entry.reason = "the rewrite could not be written";
            LOG_WARNING(Core, "RedZone: {} {:#x} not rewritten", name, result.start);
        }
        module.entries.push_back(std::move(entry));
    }
    for (u32 i = 0; i < module.entries.size(); ++i) {
        if (module.entries[i].copy != 0) {
            module.by_copy.push_back(i);
        }
    }
    std::sort(module.by_copy.begin(), module.by_copy.end(), [&](u32 a, u32 b) {
        return module.entries[a].copy < module.entries[b].copy;
    });
    module.faults = std::make_unique<std::atomic<u32>[]>(module.entries.size() + 1);

    if (!config.dump.empty()) {
        // What was written, for checking offline: the bytes at the entry, then the function as
        // it is now (in place) or its copy.
        std::ofstream out{config.dump, std::ios::app};
        const auto hex = [&](const u8* bytes, u64 count) {
            static constexpr char digits[] = "0123456789abcdef";
            for (u64 i = 0; i < count; ++i) {
                out << digits[bytes[i] >> 4] << digits[bytes[i] & 15];
            }
        };
        for (const Entry& entry : module.entries) {
            if (!entry.rewritten) {
                continue;
            }
            const bool in_place = entry.verdict == Verdict::Frame;
            out << module.name << '\t' << std::hex << entry.start - base << '\t'
                << (in_place ? "frame" : "move") << '\t' << std::dec << entry.shift << '\t'
                << std::hex << (in_place ? 0 : entry.copy - base) << std::dec << '\t';
            hex(reinterpret_cast<const u8*>(entry.start), 5);
            out << '\t';
            if (in_place) {
                hex(reinterpret_cast<const u8*>(entry.start), entry.size);
            } else {
                hex(reinterpret_cast<const u8*>(entry.copy), entry.copy_size);
            }
            out << '\n';
        }
    }
    if (!config.report.empty()) {
        std::ofstream out{config.report, std::ios::app};
        for (const Entry& entry : module.entries) {
            const char* verdict = entry.verdict == Verdict::Frame   ? "frame"
                                  : entry.verdict == Verdict::Move ? "move"
                                                                   : entry.reason;
            out << module.name << '\t' << std::hex << entry.start - base << std::dec << '\t'
                << entry.size << '\t' << entry.lowest << '\t' << verdict << '\n';
        }
    }
    const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
                             std::chrono::steady_clock::now() - started)
                             .count();
    LOG_INFO(Core,
             "RedZone: {}: {} functions, {} without calls keep data below RSP: {} rewritten in "
             "place, {} moved, {} left alone{} ({} ms)",
             name, functions.size(), results.size(), in_place, moved, left,
             config.rewrite ? "" : " (rewriting is off)", elapsed);
    if (config.stats) {
        module.functions = functions;
        module.function_faults = std::make_unique<std::atomic<u32>[]>(functions.size() + 1);
    }
    {
        std::unique_lock lock{registry_mutex};
        std::erase_if(registry, [&](const ModuleEntries& other) {
            return other.base < base + size && base < other.base + other.size;
        });
        registry.push_back(std::move(module));
    }
    if (name == "eboot.bin" && !config.probe.empty()) {
        InstallProbes(image, base);
    }
}

u64 CurrentAddress(u64 address) {
    std::shared_lock lock{registry_mutex};
    const Found found = Find(address);
    if (found.entry == nullptr || found.in_copy || found.entry->copy == 0) {
        return address;
    }
    const Entry& entry = *found.entry;
    const u32 offset = static_cast<u32>(address - entry.start);
    const auto it = std::lower_bound(entry.old_offsets.begin(), entry.old_offsets.end(), offset);
    if (it == entry.old_offsets.end() || *it != offset) {
        return address;
    }
    return entry.copy + entry.new_offsets[it - entry.old_offsets.begin()];
}

void OnHandledFault(u64 rip) {
    if (!GetConfig().stats) {
        return;
    }
    ++faults_total;
    std::shared_lock lock{registry_mutex};
    const Found found = Find(rip);
    if (found.module != nullptr) {
        ++faults_guest;
        const auto& functions = found.module->functions;
        const u64 offset = rip - found.module->base;
        auto it = std::upper_bound(functions.begin(), functions.end(), offset,
                                   [](u64 value, const Function& fn) { return value < fn.start; });
        if (!found.in_copy && it != functions.begin() &&
            offset < (it - 1)->start + (it - 1)->size) {
            ++found.module->function_faults[it - 1 - functions.begin()];
        } else if (found.entry == nullptr) {
            ++faults_unknown_code;
        }
    }
    if (found.entry != nullptr) {
        ++(found.entry->rewritten ? faults_protected : faults_exposed);
        const u32 count = ++found.module->faults[found.index];
        if ((count & (count - 1)) == 0) {
            LOG_INFO(Core, "RedZone stats: exception {} inside {} {:#x} ({}), at {:#x}", count,
                     found.module->name, found.entry->start - found.module->base,
                     VerdictName(*found.entry), rip);
        }
    }
    const s64 now = std::chrono::duration_cast<std::chrono::seconds>(
                        std::chrono::steady_clock::now().time_since_epoch())
                        .count();
    s64 last = last_dump.load();
    if (now - last >= 10 && last_dump.compare_exchange_strong(last, now)) {
        DumpStats();
    }
}

bool HandleProbe(void* exception_pointers) {
    if (probes.empty()) {
        return false;
    }
    auto* exception = static_cast<EXCEPTION_POINTERS*>(exception_pointers);
    const auto* address = static_cast<const u8*>(exception->ExceptionRecord->ExceptionAddress);
    for (Probe& probe : probes) {
        if (probe.breakpoint != address) {
            continue;
        }
        exception->ContextRecord->Rip = reinterpret_cast<u64>(address) + 1;
        const u64 hits = ++probe.hits;
        if ((hits & (hits - 1)) == 0) {
            LOG_CRITICAL(Core,
                         "RedZone probe {:#x}: {} handled exceptions so far, data below RSP came "
                         "back changed after {}",
                         probe.vaddr, hits, probe.changed);
        }
        return true;
    }
    return false;
}

} // namespace Core::RedZone

#else

namespace Core::RedZone {
void OnModuleLoaded(std::string_view, u64, u64, u64, u64) {}
u64 CurrentAddress(u64 address) {
    return address;
}
void OnHandledFault(u64) {}
bool HandleProbe(void*) {
    return false;
}
} // namespace Core::RedZone

#endif
