// SPDX-FileCopyrightText: Copyright 2025 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <map>
#include <thread>
#include <ranges>
#include <magic_enum/magic_enum.hpp>

#include "common/assert.h"
#include "common/error.h"
#include "common/logging/log.h"
#include "common/scope_exit.h"
#include "common/staged_file_read.h"
#include "common/singleton.h"
#include "core/file_sys/devices/console_device.h"
#include "core/file_sys/devices/deci_tty6_device.h"
#include "core/file_sys/devices/logger.h"
#include "core/file_sys/devices/nop_device.h"
#include "core/file_sys/devices/random_device.h"
#include "core/file_sys/devices/rng_device.h"
#include "core/file_sys/devices/srandom_device.h"
#include "core/file_sys/devices/urandom_device.h"
#include "core/file_sys/directories/normal_directory.h"
#include "core/file_sys/directories/pfs_directory.h"
#include "core/file_sys/fs.h"
#include "core/libraries/kernel/file_system.h"
#include "core/libraries/kernel/orbis_error.h"
#include "core/libraries/kernel/posix_error.h"
#include "core/libraries/libs.h"
#include "core/libraries/net/net_kernel.h"
#include "core/memory.h"
#include "kernel.h"

#ifdef _WIN32
#include <io.h>
#include <winsock2.h>
#else
#include <sys/select.h>
#include <sys/stat.h>
#endif

namespace D = Core::Devices;
namespace fs = std::filesystem;
using FactoryDevice = std::function<std::shared_ptr<D::BaseDevice>(u32, const char*, int, u16)>;

#define GET_DEVICE_FD(fd)                                                                          \
    [](u32, const char*, int, u16) {                                                               \
        return Common::Singleton<Core::FileSys::HandleTable>::Instance()->GetFile(fd)->device;     \
    }

// prefix path, only dev devices
static std::map<std::string, FactoryDevice> available_device = {
    // clang-format off
    {"/dev/stdin", GET_DEVICE_FD(0)},
    {"/dev/stdout", GET_DEVICE_FD(1)},
    {"/dev/stderr", GET_DEVICE_FD(2)},

    {"/dev/fd/0", GET_DEVICE_FD(0)},
    {"/dev/fd/1", GET_DEVICE_FD(1)},
    {"/dev/fd/2", GET_DEVICE_FD(2)},

    {"/dev/deci_stdin", GET_DEVICE_FD(0)},
    {"/dev/deci_stdout", GET_DEVICE_FD(1)},
    {"/dev/deci_stderr", GET_DEVICE_FD(2)},

    {"/dev/null", GET_DEVICE_FD(0)}, // fd0 (stdin) is a nop device

    {"/dev/urandom",  &D::URandomDevice::Create },
    {"/dev/random",   &D::RandomDevice::Create },
    {"/dev/srandom",  &D::SRandomDevice::Create },
    {"/dev/console",  &D::ConsoleDevice::Create },
    {"/dev/deci_tty6",&D::DeciTty6Device::Create },
    {"/dev/rng",      &D::RngDevice::Create },
    // clang-format on
};

namespace Libraries::Kernel {

s32 PS4_SYSV_ABI open(const char* raw_path, s32 flags, u16 mode) {
    LOG_INFO(Kernel_Fs, "path = {} flags = {:#x} mode = {:#o}", raw_path, flags, mode);

    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();

    bool read = (flags & 0x3) == ORBIS_KERNEL_O_RDONLY;
    bool write = (flags & 0x3) == ORBIS_KERNEL_O_WRONLY;
    bool rdwr = (flags & 0x3) == ORBIS_KERNEL_O_RDWR;

    if (!read && !write && !rdwr) {
        // Start by checking for invalid flags.
        *__Error() = POSIX_EINVAL;
        LOG_ERROR(Kernel_Fs, "Opening path {} failed, invalid flags {:#x}", raw_path, flags);
        return -1;
    }

    if (strlen(raw_path) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        LOG_ERROR(Kernel_Fs, "Opening path {} failed, path is too long", raw_path);
        return -1;
    }

    bool nonblock = (flags & ORBIS_KERNEL_O_NONBLOCK) != 0;
    bool append = (flags & ORBIS_KERNEL_O_APPEND) != 0;
    // Flags fsync and sync behave the same
    bool sync = (flags & ORBIS_KERNEL_O_SYNC) != 0 || (flags & ORBIS_KERNEL_O_FSYNC) != 0;
    bool create = (flags & ORBIS_KERNEL_O_CREAT) != 0;
    bool truncate = (flags & ORBIS_KERNEL_O_TRUNC) != 0;
    bool excl = (flags & ORBIS_KERNEL_O_EXCL) != 0;
    bool dsync = (flags & ORBIS_KERNEL_O_DSYNC) != 0;
    bool direct = (flags & ORBIS_KERNEL_O_DIRECT) != 0;
    bool directory = (flags & ORBIS_KERNEL_O_DIRECTORY) != 0;

    if (sync || direct || dsync || nonblock) {
        LOG_WARNING(Kernel_Fs, "flags {:#x} not fully handled", flags);
    }

    std::string_view path{raw_path};
    u32 handle = h->CreateHandle();
    auto* file = h->GetFile(handle);

    if (path.starts_with("/dev/")) {
        for (const auto& [prefix, factory] : available_device) {
            if (path.starts_with(prefix)) {
                file->is_opened = true;
                file->type = Core::FileSys::FileType::Device;
                file->m_guest_name = path;
                file->device = factory(handle, path.data(), flags, mode);
                return handle;
            }
        }
    }

    bool read_only = false;
    file->m_guest_name = path;
    file->m_host_name = mnt->GetHostPath(file->m_guest_name, &read_only);
    bool exists = fs::exists(file->m_host_name);
    s32 e = 0;

    if (create) {
        if (excl && exists) {
            // Error if file exists
            h->DeleteHandle(handle);
            *__Error() = POSIX_EEXIST;
            LOG_ERROR(Kernel_Fs, "Creating {} failed, file already exists", raw_path);
            return -1;
        }

        if (!exists) {
            if (read_only) {
                // Can't create files in a read only directory
                h->DeleteHandle(handle);
                *__Error() = POSIX_EROFS;
                LOG_ERROR(Kernel_Fs, "Creating {} failed, path is read-only", raw_path);
                return -1;
            }
            // Create a file if it doesn't exist
            Common::FS::IOFile out(file->m_host_name, Common::FS::FileAccessMode::Create);
        }
    } else if (!exists) {
        // If we're not creating a file, and it doesn't exist, return ENOENT
        h->DeleteHandle(handle);
        *__Error() = POSIX_ENOENT;
        LOG_ERROR(Kernel_Fs, "Opening path {} failed, file does not exist", raw_path);
        return -1;
    }

    if (fs::is_directory(file->m_host_name) || directory) {
        // Directories can be opened even if the directory flag isn't set.
        // In these cases, error behavior is identical to the directory code path.
        directory = true;
    }

    if (directory) {
        if (!fs::is_directory(file->m_host_name)) {
            // If the opened file is not a directory, return ENOTDIR.
            // This will trigger when create & directory is specified, this is expected.
            h->DeleteHandle(handle);
            *__Error() = POSIX_ENOTDIR;
            LOG_ERROR(Kernel_Fs, "Opening directory {} failed, file is not a directory", raw_path);
            return -1;
        }

        if (write || rdwr) {
            // Cannot open directories with any type of write access
            h->DeleteHandle(handle);
            *__Error() = POSIX_EISDIR;
            LOG_ERROR(Kernel_Fs, "Opening directory {} failed, cannot open directories for writing",
                      raw_path);
            return -1;
        }

        if (truncate) {
            // Cannot open directories with truncate
            h->DeleteHandle(handle);
            *__Error() = POSIX_EISDIR;
            LOG_ERROR(Kernel_Fs, "Opening directory {} failed, cannot truncate directories",
                      raw_path);
            return -1;
        }

        file->type = Core::FileSys::FileType::Directory;
        file->is_opened = true;
        if (file->m_guest_name.starts_with("/app0")) {
            // TODO: Properly identify type for paths like "/app0/.."
            file->directory = Core::Directories::PfsDirectory::Create(file->m_guest_name);
        } else {
            file->directory = Core::Directories::NormalDirectory::Create(file->m_guest_name);
        }
    } else {
        file->type = Core::FileSys::FileType::Regular;

        if (truncate && read_only) {
            // Can't open files with truncate flag in a read only directory
            h->DeleteHandle(handle);
            *__Error() = POSIX_EROFS;
            LOG_ERROR(Kernel_Fs, "Truncating {} failed, path is read-only", raw_path);
            return -1;
        } else if (truncate) {
            // Open the file as read-write so we can truncate regardless of flags.
            // Since open starts by closing the file, this won't interfere with later open calls.
            e = file->f.Open(file->m_host_name, Common::FS::FileAccessMode::ReadWrite);
            if (e == 0) {
                // If the file was opened successfully, reduce size to 0
                file->f.SetSize(0);
            }
        }

        if (read) {
            // Open exclusively for reading
            e = file->f.Open(file->m_host_name, Common::FS::FileAccessMode::Read);
        } else if (read_only) {
            // Can't open files with write/read-write access in a read only directory
            h->DeleteHandle(handle);
            *__Error() = POSIX_EROFS;
            LOG_ERROR(Kernel_Fs, "Opening {} for writing failed, path is read-only", raw_path);
            return -1;
        } else if (write) {
            if (append) {
                // Open exclusively for appending
                e = file->f.Open(file->m_host_name, Common::FS::FileAccessMode::Append);
            } else {
                // Open exclusively for writing
                e = file->f.Open(file->m_host_name, Common::FS::FileAccessMode::Write);
            }
        } else if (rdwr) {
            // Read and write
            if (append) {
                // Open for reading and appending
                e = file->f.Open(file->m_host_name, Common::FS::FileAccessMode::ReadAppend);
            } else {
                // Open for reading and writing
                e = file->f.Open(file->m_host_name, Common::FS::FileAccessMode::ReadWrite);
            }
        }
    }

    if (e != 0) {
        // Open failed in platform-specific code, errno needs to be converted.
        h->DeleteHandle(handle);
        SetPosixErrno(e);
        LOG_ERROR(Kernel_Fs, "Opening {} failed, error = {}", raw_path, *__Error());
        return -1;
    }

    file->is_opened = true;
    return handle;
}

s32 PS4_SYSV_ABI posix_open(const char* filename, s32 flags, u16 mode) {
    return open(filename, flags, mode);
}

s32 PS4_SYSV_ABI sceKernelOpen(const char* path, s32 flags, /* SceKernelMode*/ u16 mode) {
    s32 result = open(path, flags, mode);
    if (result < 0) {
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI close(s32 fd) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }
    if (fd < 3) {
        *__Error() = POSIX_EPERM;
        return -1;
    }
    if (file->type == Core::FileSys::FileType::Socket) {
        return Libraries::Net::KernelClose(fd);
    }
    if (file->type == Core::FileSys::FileType::Regular) {
        file->f.Close();
    }
    file->is_opened = false;
    LOG_INFO(Kernel_Fs, "Closing {}", file->m_guest_name);
    // FIXME: Lock file mutex before deleting it?
    h->DeleteHandle(fd);
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI posix_close(s32 fd) {
    return close(fd);
}

s32 PS4_SYSV_ABI sceKernelClose(s32 fd) {
    s32 result = close(fd);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI write(s32 fd, const void* buf, u64 nbytes) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};
    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->write(buf, nbytes);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Socket) {
        return Libraries::Net::KernelWrite(fd, buf, nbytes);
    } else if (file->type == Core::FileSys::FileType::Directory) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    return file->f.WriteRaw<u8>(buf, nbytes);
}

s64 PS4_SYSV_ABI posix_write(s32 fd, const void* buf, u64 nbytes) {
    return write(fd, buf, nbytes);
}

s64 PS4_SYSV_ABI sceKernelWrite(s32 fd, const void* buf, u64 nbytes) {
    s64 result = write(fd, buf, nbytes);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 ReadFile(Common::FS::IOFile& file, void* buf, u64 nbytes) {
    const auto* memory = Core::Memory::Instance();
    // Invalidate up to the actual number of bytes that could be read.
    const auto position = file.Tell();
    const auto file_size = file.GetSize();
    const auto remaining = position >= 0 && static_cast<u64>(position) < file_size
                               ? file_size - static_cast<u64>(position) : 0;
    memory->InvalidateMemory(reinterpret_cast<VAddr>(buf), std::min<u64>(nbytes, remaining));

#ifdef _WIN32
    // Windows rejects reads into GPU-tracked guest pages with
    // ERROR_INVALID_USER_BUFFER (1784), without invoking the fault handler.
    return Common::FS::ReadStaged(buf, nbytes, [&](void* staging, size_t size) {
        return file.ReadRaw<u8>(staging, size);
    });
#else
    return file.ReadRaw<u8>(buf, nbytes);
#endif
}

s64 PS4_SYSV_ABI readv(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};
    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->readv(iov, iovcnt);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Directory) {
        s64 result = file->directory->readv(iov, iovcnt);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }

    if (file->f.IsWriteOnly()) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    s64 total_read = 0;
    for (s32 i = 0; i < iovcnt; i++) {
        total_read += ReadFile(file->f, iov[i].iov_base, iov[i].iov_len);
    }
    return total_read;
}

s64 PS4_SYSV_ABI posix_readv(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt) {
    return readv(fd, iov, iovcnt);
}

s64 PS4_SYSV_ABI sceKernelReadv(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt) {
    s64 result = readv(fd, iov, iovcnt);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI writev(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};

    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->writev(iov, iovcnt);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Directory) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    s64 total_written = 0;
    for (s32 i = 0; i < iovcnt; i++) {
        total_written += file->f.WriteRaw<u8>(iov[i].iov_base, iov[i].iov_len);
    }
    return total_written;
}

s64 PS4_SYSV_ABI posix_writev(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt) {
    return writev(fd, iov, iovcnt);
}

s64 PS4_SYSV_ABI sceKernelWritev(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt) {
    s64 result = writev(fd, iov, iovcnt);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI posix_lseek(s32 fd, s64 offset, s32 whence) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};
    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->lseek(offset, whence);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Directory) {
        s64 result = file->directory->lseek(offset, whence);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }

    Common::FS::SeekOrigin origin{};
    if (whence == 0) {
        origin = Common::FS::SeekOrigin::SetOrigin;
    } else if (whence == 1) {
        origin = Common::FS::SeekOrigin::CurrentPosition;
    } else if (whence == 2) {
        origin = Common::FS::SeekOrigin::End;
    } else if (whence == 3 || whence == 4) {
        // whence parameter belongs to an unsupported POSIX extension
        *__Error() = POSIX_ENOTTY;
        return -1;
    } else {
        // whence parameter is invalid
        *__Error() = POSIX_EINVAL;
        return -1;
    }

    if (!file->f.Seek(offset, origin)) {
        if (errno != 0) {
            // Seek failed in platform-specific code, errno needs to be converted.
            SetPosixErrno(errno);
            return -1;
        }
        // Shouldn't be possible, but just in case.
        return -1;
    }

    s64 result = file->f.Tell();
    if (result < 0) {
        // Tell failed in platform-specific code, errno needs to be converted.
        SetPosixErrno(errno);
        return -1;
    }
    return result;
}

s64 PS4_SYSV_ABI sceKernelLseek(s32 fd, s64 offset, s32 whence) {
    s64 result = posix_lseek(fd, offset, whence);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI read(s32 fd, void* buf, u64 nbytes) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};
    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->read(buf, nbytes);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Directory) {
        s64 result = file->directory->read(buf, nbytes);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Socket) {
        return Libraries::Net::KernelRead(fd, buf, nbytes);
    }

    if (file->f.IsWriteOnly()) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    return ReadFile(file->f, buf, nbytes);
}

s64 PS4_SYSV_ABI posix_read(s32 fd, void* buf, u64 nbytes) {
    return read(fd, buf, nbytes);
}

s64 PS4_SYSV_ABI sceKernelRead(s32 fd, void* buf, u64 nbytes) {
    s64 result = read(fd, buf, nbytes);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI posix_mkdir(const char* path, u16 mode) {
    LOG_INFO(Kernel_Fs, "path = {} mode = {:#o}", path, mode);
    if (strlen(path) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        return -1;
    }
    if (path == nullptr) {
        *__Error() = POSIX_ENOTDIR;
        return -1;
    }
    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();

    bool ro = false;
    const auto dir_name = mnt->GetHostPath(path, &ro);

    if (fs::exists(dir_name)) {
        *__Error() = POSIX_EEXIST;
        return -1;
    }

    if (ro) {
        *__Error() = POSIX_EROFS;
        return -1;
    }

    // CUSA02456: path = /aotl after sceSaveDataMount(mode = 1)
    std::error_code ec;
    if (dir_name.empty() || !fs::create_directory(dir_name, ec)) {
        *__Error() = POSIX_EIO;
        return -1;
    }

    if (!fs::exists(dir_name)) {
        *__Error() = POSIX_ENOENT;
        return -1;
    }
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelMkdir(const char* path, u16 mode) {
    s32 result = posix_mkdir(path, mode);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI posix_rmdir(const char* path) {
    if (strlen(path) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        return -1;
    }
    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();
    bool ro = false;

    const fs::path dir_name = mnt->GetHostPath(path, &ro);

    if (ro) {
        *__Error() = POSIX_EROFS;
        return -1;
    }

    if (dir_name.empty() || !fs::is_directory(dir_name)) {
        *__Error() = POSIX_ENOTDIR;
        return -1;
    }

    if (!fs::exists(dir_name)) {
        *__Error() = POSIX_ENOENT;
        return -1;
    }

    std::error_code ec;
    s32 result = fs::remove_all(dir_name, ec);

    if (ec) {
        *__Error() = POSIX_EIO;
        return -1;
    }
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelRmdir(const char* path) {
    s32 result = posix_rmdir(path);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI posix_stat(const char* path, OrbisKernelStat* sb) {
    LOG_DEBUG(Kernel_Fs, "(PARTIAL) path = {}", path);
    if (strlen(path) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        return -1;
    }
    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();
    const auto path_name = mnt->GetHostPath(path);
    std::memset(sb, 0, sizeof(OrbisKernelStat));
    const bool is_dir = fs::is_directory(path_name);
    const bool is_file = fs::is_regular_file(path_name);
    if (!is_dir && !is_file) {
        *__Error() = POSIX_ENOENT;
        return -1;
    }

    // get the difference between file clock and system clock
    const auto now_sys = std::chrono::system_clock::now();
    const auto now_file = fs::file_time_type::clock::now();
    // calculate the file modified time
    const auto mtime = fs::last_write_time(path_name);
    const auto mtimestamp = now_sys + (mtime - now_file);

    if (fs::is_directory(path_name)) {
        sb->st_mode = 0000777u | 0040000u;
        sb->st_size = 65536;
        sb->st_blksize = 65536;
        sb->st_blocks = 128;
        sb->st_mtim.tv_sec =
            std::chrono::duration_cast<std::chrono::seconds>(mtimestamp.time_since_epoch()).count();
        // TODO incomplete
    } else {
        sb->st_mode = 0000777u | 0100000u;
        sb->st_size = static_cast<s64>(fs::file_size(path_name));
        sb->st_blksize = 512;
        sb->st_blocks = (sb->st_size + 511) / 512;
        sb->st_mtim.tv_sec =
            std::chrono::duration_cast<std::chrono::seconds>(mtimestamp.time_since_epoch()).count();
        // TODO incomplete
    }

    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelStat(const char* path, OrbisKernelStat* sb) {
    s32 result = posix_stat(path, sb);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI sceKernelCheckReachability(const char* path) {
    if (strlen(path) > 255) {
        return ORBIS_KERNEL_ERROR_ENAMETOOLONG;
    }

    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();
    std::string_view guest_path{path};
    for (const auto& prefix : available_device | std::views::keys) {
        if (guest_path.starts_with(prefix)) {
            return ORBIS_OK;
        }
    }
    const auto path_name = mnt->GetHostPath(guest_path);
    if (!fs::exists(path_name)) {
        return ORBIS_KERNEL_ERROR_ENOENT;
    }
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI fstat(s32 fd, OrbisKernelStat* sb) {
    LOG_DEBUG(Kernel_Fs, "(PARTIAL) fd = {}", fd);
    if (sb == nullptr) {
        *__Error() = POSIX_EFAULT;
        return -1;
    }
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }
    std::memset(sb, 0, sizeof(OrbisKernelStat));

    switch (file->type) {
    case Core::FileSys::FileType::Device: {
        s32 result = file->device->fstat(sb);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }
    case Core::FileSys::FileType::Regular: {
        sb->st_mode = 0000777u | 0100000u;
        sb->st_size = file->f.GetSize();
        sb->st_blksize = 512;
        sb->st_blocks = (sb->st_size + 511) / 512;
#if defined(__linux__) || defined(__FreeBSD__)
        struct stat filestat = {};
        stat(file->f.GetPath().c_str(), &filestat);
        sb->st_atim = *reinterpret_cast<OrbisKernelTimespec*>(&filestat.st_atim);
        sb->st_mtim = *reinterpret_cast<OrbisKernelTimespec*>(&filestat.st_mtim);
        sb->st_ctim = *reinterpret_cast<OrbisKernelTimespec*>(&filestat.st_ctim);
#elif defined(__APPLE__)
        struct stat filestat = {};
        stat(file->f.GetPath().c_str(), &filestat);
        sb->st_atim = *reinterpret_cast<OrbisKernelTimespec*>(&filestat.st_atimespec);
        sb->st_mtim = *reinterpret_cast<OrbisKernelTimespec*>(&filestat.st_mtimespec);
        sb->st_ctim = *reinterpret_cast<OrbisKernelTimespec*>(&filestat.st_ctimespec);
#else
        const auto ft = std::filesystem::last_write_time(file->f.GetPath());
        const auto sctp = std::chrono::time_point_cast<std::chrono::nanoseconds>(
            ft - std::filesystem::file_time_type::clock::now() + std::chrono::system_clock::now());
        const auto secs = std::chrono::time_point_cast<std::chrono::seconds>(sctp);
        const auto nsecs = std::chrono::duration_cast<std::chrono::nanoseconds>(sctp - secs);

        sb->st_mtim.tv_sec = static_cast<int64_t>(secs.time_since_epoch().count());
        sb->st_mtim.tv_nsec = static_cast<int64_t>(nsecs.count());
        sb->st_atim = sb->st_mtim;
        sb->st_ctim = sb->st_mtim;
#endif
        // TODO incomplete
        break;
    }
    case Core::FileSys::FileType::Directory: {
        s32 result = file->directory->fstat(sb);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }
    case Core::FileSys::FileType::Socket: {
        return Libraries::Net::KernelFstat(fd, sb);
    }
    case Core::FileSys::FileType::Epoll:
    case Core::FileSys::FileType::Resolver:
    case Core::FileSys::FileType::Equeue: {
        LOG_ERROR(Kernel_Fs, "(STUBBED) file type {}", magic_enum::enum_name(file->type.load()));
        break;
    }
    default:
        UNREACHABLE_MSG("{}", u32(file->type.load()));
    }
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI posix_fstat(s32 fd, OrbisKernelStat* sb) {
    return fstat(fd, sb);
}

s32 PS4_SYSV_ABI sceKernelFstat(s32 fd, OrbisKernelStat* sb) {
    s32 result = fstat(fd, sb);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI posix_ftruncate(s32 fd, s64 length) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);

    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    if (file->type == Core::FileSys::FileType::Device) {
        s32 result = file->device->ftruncate(length);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }

    if (file->m_host_name.empty()) {
        *__Error() = POSIX_EACCES;
        return -1;
    }

    file->f.SetSize(length);
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelFtruncate(s32 fd, s64 length) {
    s32 result = posix_ftruncate(fd, length);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI posix_rename(const char* from, const char* to) {
    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();
    bool ro = false;
    const auto src_path = mnt->GetHostPath(from, &ro);
    if (strlen(from) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        return -1;
    }
    if (strlen(to) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        return -1;
    }
    if (!fs::exists(src_path)) {
        *__Error() = POSIX_ENOENT;
        return -1;
    }
    if (ro) {
        *__Error() = POSIX_EROFS;
        return -1;
    }
    const auto dst_path = mnt->GetHostPath(to, &ro);
    if (ro) {
        *__Error() = POSIX_EROFS;
        return -1;
    }
    const bool src_is_dir = fs::is_directory(src_path);
    const bool dst_is_dir = fs::is_directory(dst_path);

    if (fs::exists(dst_path)) {
        if (src_is_dir && !dst_is_dir) {
            *__Error() = POSIX_ENOTDIR;
            return -1;
        }
        if (!src_is_dir && dst_is_dir) {
            *__Error() = POSIX_EISDIR;
            return -1;
        }
        if (dst_is_dir && !fs::is_empty(dst_path)) {
            *__Error() = POSIX_ENOTEMPTY;
            return -1;
        }
    }

    // On Windows, fs::rename will error if the file has been opened before.
    fs::copy(src_path, dst_path,
             fs::copy_options::overwrite_existing | fs::copy_options::recursive);
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto file = h->GetFile(src_path);
    if (file) {
        auto access_mode = file->f.GetAccessMode();
        file->f.Close();
        fs::remove(src_path);
        file->f.Open(dst_path, access_mode);
    } else {
        fs::remove_all(src_path);
    }

    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelRename(const char* from, const char* to) {
    s32 result = posix_rename(from, to);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI posix_preadv(s32 fd, OrbisKernelIovec* iov, s32 iovcnt, s64 offset) {
    if (offset < 0) {
        *__Error() = POSIX_EINVAL;
        return -1;
    }

    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};
    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->preadv(iov, iovcnt, offset);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Directory) {
        s64 result = file->directory->preadv(iov, iovcnt, offset);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }

    if (file->f.IsWriteOnly()) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    const s64 pos = file->f.Tell();
    SCOPE_EXIT {
        file->f.Seek(pos);
    };
    if (!file->f.Seek(offset)) {
        *__Error() = POSIX_EIO;
        return -1;
    }
    s64 total_read = 0;
    for (s32 i = 0; i < iovcnt; i++) {
        total_read += ReadFile(file->f, iov[i].iov_base, iov[i].iov_len);
    }
    return total_read;
}

s64 PS4_SYSV_ABI sceKernelPreadv(s32 fd, OrbisKernelIovec* iov, s32 iovcnt, s64 offset) {
    s64 result = posix_preadv(fd, iov, iovcnt, offset);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI posix_pread(s32 fd, void* buf, u64 nbytes, s64 offset) {
    OrbisKernelIovec iovec{buf, nbytes};
    return posix_preadv(fd, &iovec, 1, offset);
}

s64 PS4_SYSV_ABI sceKernelPread(s32 fd, void* buf, u64 nbytes, s64 offset) {
    OrbisKernelIovec iovec{buf, nbytes};
    return sceKernelPreadv(fd, &iovec, 1, offset);
}

s32 PS4_SYSV_ABI posix_fsync(s32 fd) {
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    if (file->type == Core::FileSys::FileType::Device) {
        s32 result = file->device->fsync();
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }
    file->f.Flush();
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelFsync(s32 fd) {
    s32 result = posix_fsync(fd);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

static s64 GetDents(s32 fd, char* buf, u64 nbytes, s64* basep) {
    if (buf == nullptr) {
        *__Error() = POSIX_EFAULT;
        return -1;
    }
    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    if (nbytes < 512) {
        *__Error() = POSIX_EINVAL;
        return -1;
    }

    switch (file->type) {
    case Core::FileSys::FileType::Directory: {
        s64 result = file->directory->getdents(buf, nbytes, basep);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }
    case Core::FileSys::FileType::Device: {
        s64 result = file->device->getdents(buf, nbytes, basep);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    }
    default: {
        // Not directory or device
        *__Error() = POSIX_EINVAL;
        return -1;
    }
    }

    return ORBIS_OK;
}

s64 PS4_SYSV_ABI posix_getdents(s32 fd, char* buf, u64 nbytes) {
    return GetDents(fd, buf, nbytes, nullptr);
}

s64 PS4_SYSV_ABI sceKernelGetdents(s32 fd, char* buf, u64 nbytes) {
    s64 result = posix_getdents(fd, buf, nbytes);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI getdirentries(s32 fd, char* buf, u64 nbytes, s64* basep) {
    return GetDents(fd, buf, nbytes, basep);
}

s64 PS4_SYSV_ABI posix_getdirentries(s32 fd, char* buf, u64 nbytes, s64* basep) {
    return GetDents(fd, buf, nbytes, basep);
}

s64 PS4_SYSV_ABI sceKernelGetdirentries(s32 fd, char* buf, u64 nbytes, s64* basep) {
    s64 result = GetDents(fd, buf, nbytes, basep);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI posix_pwritev(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt, s64 offset) {
    if (offset < 0) {
        *__Error() = POSIX_EINVAL;
        return -1;
    }

    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* file = h->GetFile(fd);
    if (file == nullptr) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    std::scoped_lock lk{file->m_mutex};

    if (file->type == Core::FileSys::FileType::Device) {
        s64 result = file->device->pwritev(iov, iovcnt, offset);
        if (result < 0) {
            ErrSceToPosix(result);
            return -1;
        }
        return result;
    } else if (file->type == Core::FileSys::FileType::Directory) {
        *__Error() = POSIX_EBADF;
        return -1;
    }

    const s64 pos = file->f.Tell();
    SCOPE_EXIT {
        file->f.Seek(pos);
    };
    if (!file->f.Seek(offset)) {
        *__Error() = POSIX_EIO;
        return -1;
    }
    s64 total_written = 0;
    for (s32 i = 0; i < iovcnt; i++) {
        total_written += file->f.WriteRaw<u8>(iov[i].iov_base, iov[i].iov_len);
    }
    return total_written;
}

s64 PS4_SYSV_ABI posix_pwrite(s32 fd, void* buf, u64 nbytes, s64 offset) {
    OrbisKernelIovec iovec{buf, nbytes};
    return posix_pwritev(fd, &iovec, 1, offset);
}

s64 PS4_SYSV_ABI sceKernelPwrite(s32 fd, void* buf, u64 nbytes, s64 offset) {
    s64 result = posix_pwrite(fd, buf, nbytes, offset);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s64 PS4_SYSV_ABI sceKernelPwritev(s32 fd, const OrbisKernelIovec* iov, s32 iovcnt, s64 offset) {
    s64 result = posix_pwritev(fd, iov, iovcnt, offset);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

s32 PS4_SYSV_ABI posix_unlink(const char* path) {
    if (strlen(path) > 255) {
        *__Error() = POSIX_ENAMETOOLONG;
        return -1;
    }
    if (path == nullptr) {
        *__Error() = POSIX_EINVAL;
        return -1;
    }

    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    auto* mnt = Common::Singleton<Core::FileSys::MntPoints>::Instance();

    bool ro = false;
    const auto host_path = mnt->GetHostPath(path, &ro);
    if (host_path.empty()) {
        *__Error() = POSIX_ENOENT;
        return -1;
    }

    if (ro) {
        *__Error() = POSIX_EROFS;
        return -1;
    }

    if (fs::is_directory(host_path)) {
        *__Error() = POSIX_EPERM;
        return -1;
    }

    auto* file = h->GetFile(host_path);
    if (file == nullptr) {
        // File to unlink hasn't been opened, manually open and unlink it.
        Common::FS::IOFile file(host_path, Common::FS::FileAccessMode::ReadWrite);
        file.Unlink();
    } else {
        file->f.Unlink();
    }

    LOG_INFO(Kernel_Fs, "Unlinked {}", path);
    return ORBIS_OK;
}

s32 PS4_SYSV_ABI sceKernelUnlink(const char* path) {
    s32 result = posix_unlink(path);
    if (result < 0) {
        LOG_ERROR(Kernel_Fs, "error = {}", *__Error());
        return ErrnoToSceKernelError(*__Error());
    }
    return result;
}

// posix_select from upstream's network stack: sockets go through Libraries::Net::KernelSelect.
namespace {

struct GuestFdSet {
    u64 bits[1024 / 64];
};
constexpr s32 kGuestFdSetSize = 1024;

bool GuestFdIsSet(const GuestFdSet* set, s32 fd) {
    return set != nullptr && (set->bits[fd / 64] >> (fd % 64) & 1) != 0;
}

void GuestFdSetBit(GuestFdSet* set, s32 fd) {
    set->bits[fd / 64] |= 1ull << (fd % 64);
}

} // namespace

s32 PS4_SYSV_ABI posix_select(s32 nfds, GuestFdSet* readfds, GuestFdSet* writefds,
                              GuestFdSet* exceptfds, OrbisKernelTimeval* timeout) {
    LOG_DEBUG(Kernel_Fs, "nfds = {}, readfds = {}, writefds = {}, exceptfds = {}, timeout = {}",
              nfds, fmt::ptr(readfds), fmt::ptr(writefds), fmt::ptr(exceptfds), fmt::ptr(timeout));
    if (nfds < 0 || nfds > kGuestFdSetSize) {
        *__Error() = POSIX_EINVAL;
        return -1;
    }
    s64 timeout_us = -1; // no limit
    if (timeout != nullptr) {
        if (timeout->tv_sec < 0 || timeout->tv_usec < 0 || timeout->tv_usec >= 1'000'000) {
            *__Error() = POSIX_EINVAL;
            return -1;
        }
        timeout_us = timeout->tv_sec > (INT64_MAX / 1'000'000 - 1)
                         ? INT64_MAX
                         : timeout->tv_sec * 1'000'000 + timeout->tv_usec;
    }

    auto* h = Common::Singleton<Core::FileSys::HandleTable>::Instance();
    GuestFdSet read_ready{}, write_ready{}, except_ready{};
    s32 ready = 0;
    std::vector<Core::Net::SelectEntry> sockets;
    for (s32 i = 0; i < nfds; ++i) {
        const bool want_read = GuestFdIsSet(readfds, i);
        const bool want_write = GuestFdIsSet(writefds, i);
        const bool want_except = GuestFdIsSet(exceptfds, i);
        if (!want_read && !want_write && !want_except) {
            continue;
        }
        auto* file = h->GetFile(i);
        if (!file || !file->is_opened ||
            (file->type == Core::FileSys::FileType::Regular && !file->f.IsOpen())) {
            LOG_ERROR(Kernel_Fs, "posix_select: fd {} is null, closed or invalid", i);
            *__Error() = POSIX_EBADF;
            return -1;
        }
        if (file->type == Core::FileSys::FileType::Socket) {
            const u32 interest =
                (want_read ? Core::Net::Host::EvIn : 0u) |
                (want_write ? Core::Net::Host::EvOut : 0u) |
                (want_except ? (Core::Net::Host::EvErr | Core::Net::Host::EvHup) : 0u);
            if (interest != 0) {
                sockets.push_back({i, interest, 0});
            }
            continue;
        }
        // Non-socket file descriptors (regular files, directories, character devices)
        // are always considered immediately ready without blocking under POSIX.
        if (want_read) {
            GuestFdSetBit(&read_ready, i);
            ++ready;
        }
        if (want_write) {
            GuestFdSetBit(&write_ready, i);
            ++ready;
        }
    }

    if (ready > 0) {
        timeout_us = 0;
    }
    if (!sockets.empty()) {
        const s64 n = Libraries::Net::KernelSelect(sockets, timeout_us);
        if (n < 0) {
            return -1; // errno set
        }
        ready += static_cast<s32>(n);
    } else if (ready == 0 && timeout_us != 0) {
        if (timeout_us > 0) {
            std::this_thread::sleep_for(std::chrono::microseconds{timeout_us});
        } else {
            // timeout_us < 0 with no sockets to poll. Yield briefly to prevent thread deadlock
            // and starvation while maintaining heartbeats for watchdog threads.
            std::this_thread::sleep_for(std::chrono::milliseconds{10});
        }
    }

    for (const auto& e : sockets) {
        if (e.ready & Core::Net::Host::EvIn) {
            GuestFdSetBit(&read_ready, e.id);
        }
        if (e.ready & Core::Net::Host::EvOut) {
            GuestFdSetBit(&write_ready, e.id);
        }
        if (e.ready & (Core::Net::Host::EvErr | Core::Net::Host::EvHup)) {
            GuestFdSetBit(&except_ready, e.id);
        }
    }
    if (readfds) {
        *readfds = read_ready;
    }
    if (writefds) {
        *writefds = write_ready;
    }
    if (exceptfds) {
        *exceptfds = except_ready;
    }
    return ready;
}

void RegisterFileSystem(Core::Loader::SymbolsResolver* sym) {
    LIB_FUNCTION("6c3rCVE-fTU", "libkernel", 1, "libkernel", open);
    LIB_FUNCTION("wuCroIGjt2g", "libScePosix", 1, "libkernel", posix_open);
    LIB_FUNCTION("wuCroIGjt2g", "libkernel", 1, "libkernel", posix_open);
    LIB_FUNCTION("1G3lF1Gg1k8", "libkernel", 1, "libkernel", sceKernelOpen);
    LIB_FUNCTION("NNtFaKJbPt0", "libkernel", 1, "libkernel", close);
    LIB_FUNCTION("bY-PO6JhzhQ", "libScePosix", 1, "libkernel", posix_close);
    LIB_FUNCTION("bY-PO6JhzhQ", "libkernel", 1, "libkernel", posix_close);
    LIB_FUNCTION("UK2Tl2DWUns", "libkernel", 1, "libkernel", sceKernelClose);
    LIB_FUNCTION("FxVZqBAA7ks", "libkernel", 1, "libkernel", write);
    LIB_FUNCTION("FN4gaPmuFV8", "libScePosix", 1, "libkernel", posix_write);
    LIB_FUNCTION("FN4gaPmuFV8", "libkernel", 1, "libkernel", posix_write);
    LIB_FUNCTION("4wSze92BhLI", "libkernel", 1, "libkernel", sceKernelWrite);
    LIB_FUNCTION("+WRlkKjZvag", "libkernel", 1, "libkernel", readv);
    LIB_FUNCTION("YSHRBRLn2pI", "libkernel", 1, "libkernel", writev);
    LIB_FUNCTION("kAt6VDbHmro", "libkernel", 1, "libkernel", sceKernelWritev);
    LIB_FUNCTION("Oy6IpwgtYOk", "libScePosix", 1, "libkernel", posix_lseek);
    LIB_FUNCTION("Oy6IpwgtYOk", "libkernel", 1, "libkernel", posix_lseek);
    LIB_FUNCTION("oib76F-12fk", "libkernel", 1, "libkernel", sceKernelLseek);
    LIB_FUNCTION("DRuBt2pvICk", "libkernel", 1, "libkernel", read);
    LIB_FUNCTION("AqBioC2vF3I", "libScePosix", 1, "libkernel", posix_read);
    LIB_FUNCTION("AqBioC2vF3I", "libkernel", 1, "libkernel", posix_read);
    LIB_FUNCTION("Cg4srZ6TKbU", "libkernel", 1, "libkernel", sceKernelRead);
    LIB_FUNCTION("JGMio+21L4c", "libScePosix", 1, "libkernel", posix_mkdir);
    LIB_FUNCTION("JGMio+21L4c", "libkernel", 1, "libkernel", posix_mkdir);
    LIB_FUNCTION("1-LFLmRFxxM", "libkernel", 1, "libkernel", sceKernelMkdir);
    LIB_FUNCTION("c7ZnT7V1B98", "libScePosix", 1, "libkernel", posix_rmdir);
    LIB_FUNCTION("c7ZnT7V1B98", "libkernel", 1, "libkernel", posix_rmdir);
    LIB_FUNCTION("naInUjYt3so", "libkernel", 1, "libkernel", sceKernelRmdir);
    LIB_FUNCTION("E6ao34wPw+U", "libScePosix", 1, "libkernel", posix_stat);
    LIB_FUNCTION("E6ao34wPw+U", "libkernel", 1, "libkernel", posix_stat);
    LIB_FUNCTION("eV9wAD2riIA", "libkernel", 1, "libkernel", sceKernelStat);
    LIB_FUNCTION("uWyW3v98sU4", "libkernel", 1, "libkernel", sceKernelCheckReachability);
    LIB_FUNCTION("mqQMh1zPPT8", "libScePosix", 1, "libkernel", posix_fstat);
    LIB_FUNCTION("mqQMh1zPPT8", "libkernel", 1, "libkernel", posix_fstat);
    LIB_FUNCTION("kBwCPsYX-m4", "libkernel", 1, "libkernel", sceKernelFstat);
    LIB_FUNCTION("ih4CD9-gghM", "libkernel", 1, "libkernel", posix_ftruncate);
    LIB_FUNCTION("ih4CD9-gghM", "libScePosix", 1, "libkernel", posix_ftruncate);
    LIB_FUNCTION("VW3TVZiM4-E", "libkernel", 1, "libkernel", sceKernelFtruncate);
    LIB_FUNCTION("NN01qLRhiqU", "libScePosix", 1, "libkernel", posix_rename);
    LIB_FUNCTION("NN01qLRhiqU", "libkernel", 1, "libkernel", posix_rename);
    LIB_FUNCTION("52NcYU9+lEo", "libkernel", 1, "libkernel", sceKernelRename);
    LIB_FUNCTION("yTj62I7kw4s", "libkernel", 1, "libkernel", sceKernelPreadv);
    LIB_FUNCTION("ezv-RSBNKqI", "libScePosix", 1, "libkernel", posix_pread);
    LIB_FUNCTION("ezv-RSBNKqI", "libkernel", 1, "libkernel", posix_pread);
    LIB_FUNCTION("+r3rMFwItV4", "libkernel", 1, "libkernel", sceKernelPread);
    LIB_FUNCTION("juWbTNM+8hw", "libScePosix", 1, "libkernel", posix_fsync);
    LIB_FUNCTION("juWbTNM+8hw", "libkernel", 1, "libkernel", posix_fsync);
    LIB_FUNCTION("fTx66l5iWIA", "libkernel", 1, "libkernel", sceKernelFsync);
    LIB_FUNCTION("j2AIqSqJP0w", "libkernel", 1, "libkernel", sceKernelGetdents);
    LIB_FUNCTION("sfKygSjIbI8", "libkernel", 1, "libkernel", getdirentries);
    LIB_FUNCTION("2G6i6hMIUUY", "libkernel", 1, "libkernel", posix_getdents);
    LIB_FUNCTION("taRWhTJFTgE", "libkernel", 1, "libkernel", sceKernelGetdirentries);
    LIB_FUNCTION("C2kJ-byS5rM", "libkernel", 1, "libkernel", posix_pwrite);
    LIB_FUNCTION("C2kJ-byS5rM", "libScePosix", 1, "libkernel", posix_pwrite);
    LIB_FUNCTION("FCcmRZhWtOk", "libScePosix", 1, "libkernel", posix_pwritev);
    LIB_FUNCTION("FCcmRZhWtOk", "libkernel", 1, "libkernel", posix_pwritev);
    LIB_FUNCTION("nKWi-N2HBV4", "libkernel", 1, "libkernel", sceKernelPwrite);
    LIB_FUNCTION("mBd4AfLP+u8", "libkernel", 1, "libkernel", sceKernelPwritev);
    LIB_FUNCTION("VAzswvTOCzI", "libkernel", 1, "libkernel", posix_unlink);
    LIB_FUNCTION("AUXVxWeJU-A", "libkernel", 1, "libkernel", sceKernelUnlink);
    LIB_FUNCTION("T8fER+tIGgk", "libScePosix", 1, "libkernel", posix_select);
    LIB_FUNCTION("T8fER+tIGgk", "libkernel", 1, "libkernel", posix_select);
}

} // namespace Libraries::Kernel
