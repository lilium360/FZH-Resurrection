// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <string>

namespace Common {

extern const char g_version[];
extern const bool g_is_release;

extern const char g_scm_rev[];
extern const char g_scm_branch[];
extern const char g_scm_desc[];
extern const char g_scm_remote_name[];
extern const char g_scm_remote_url[];
extern const char g_scm_date[];

const std::string GetRemoteNameFromLink();

/// "shadPS4 v<version>", followed on builds that are not releases by the remote's owner, the
/// branch and the commit, as far as they are known.
std::string GetVersionLabel();

} // namespace Common
