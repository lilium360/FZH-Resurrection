// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include "common/types.h"

namespace Core::GuestTrace {

/// Sets the breakpoints named by SHADPS4_GUEST_TRACE in the eboot just loaded. No effect when
/// the variable is not set.
void Install(u64 base, u64 size);

/// Called first by the exception handler (Windows: EXCEPTION_POINTERS*). True when the exception
/// was one of the trace's own breakpoints or single steps and execution can continue.
bool HandleException(void* exception_pointers);

} // namespace Core::GuestTrace
