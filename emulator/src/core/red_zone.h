// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <string_view>
#include "common/types.h"

// Guest code is built for the SysV ABI and may keep data in the 128 bytes below RSP (the red
// zone) without reserving them. Windows delivers every exception on the faulting thread's own
// stack and overwrites that area, so an exception the emulator handles and resumes (memory
// tracking, lazily patched instructions, diagnostics) can corrupt the function it interrupted.
//
// When a guest module is loaded, its functions (one per FDE of .eh_frame) are analysed and those
// that touch memory below RSP are rewritten so that RSP stays below their data:
//  - functions that open with "push rbp; mov rbp,rsp" are changed in place to "enter N,0", and
//    every "pop rbp" that closes the frame to "leave": same lengths, same addresses of the data,
//    RSP lower by N, which is the whole depth of the frame plus the red zone;
//  - functions without a frame pointer are copied next to the module with RSP lowered at the
//    entry and raised at every exit, and their RSP-relative displacements adjusted; the entry of
//    the original becomes a jump to the copy.
// A function is rewritten only when every instruction in it is accounted for and none of them
// could tell the difference; the others are left as they are and listed in the log.
//
//   SHADPS4_RED_ZONE=0             nothing is analysed or rewritten
//   SHADPS4_RED_ZONE_STATS=1       counts the handled exceptions that interrupt such functions
//                                  (with SHADPS4_RED_ZONE=0 too: analysed, not rewritten)
//   SHADPS4_RED_ZONE_REPORT=<file> writes the functions found, to compare with another analysis
//   SHADPS4_RED_ZONE_DUMP=<file>   writes the code of every rewritten function, to check offline
//   SHADPS4_RED_ZONE_PROBE=<addr>[,<addr>...]  self-test: raises a handled exception at those
//                                  eboot addresses each time they run and counts how often the
//                                  function's data below RSP came back changed (it is then
//                                  put back, so that a run without the rewrite goes on)

namespace Core::RedZone {

/// Analyses a guest module just loaded and rewrites the functions that keep data below RSP.
/// The addresses of .eh_frame_hdr are relative to the module.
void OnModuleLoaded(std::string_view name, u64 base, u64 size, u64 eh_frame_hdr,
                    u64 eh_frame_hdr_size);

/// Where the instruction at a guest address runs now: inside the copy when its function was
/// moved, the address itself otherwise. Only kept for diagnostics that set breakpoints.
u64 CurrentAddress(u64 address);

/// Counts an exception that was handled and resumed, by the guest function it interrupted.
void OnHandledFault(u64 rip);

/// Called by the exception handler (Windows: EXCEPTION_POINTERS*). True when the breakpoint
/// belongs to SHADPS4_RED_ZONE_PROBE and execution can continue.
bool HandleProbe(void* exception_pointers);

} // namespace Core::RedZone
