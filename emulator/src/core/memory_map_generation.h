// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <array>
#include <atomic>
#include <cstdint>

namespace Core {

class MemoryMapGeneration final {
public:
    class Mutation final {
    public:
        explicit Mutation(MemoryMapGeneration& owner_) : owner{owner_} {
            owner.value.fetch_add(1, std::memory_order_acq_rel);
        }

        ~Mutation() {
            Finish();
        }

        void Finish() {
            if (!active) {
                return;
            }
            owner.value.fetch_add(1, std::memory_order_release);
            active = false;
        }

        Mutation(const Mutation&) = delete;
        Mutation& operator=(const Mutation&) = delete;

    private:
        MemoryMapGeneration& owner;
        bool active{true};
    };

    [[nodiscard]] Mutation BeginMutation() {
        return Mutation{*this};
    }

    [[nodiscard]] std::uint64_t Load() const {
        return value.load(std::memory_order_acquire);
    }

private:
    std::atomic_uint64_t value{};
};

/// The last ranges of guest memory that may have stopped being mapped, and the last ones that
/// were mapped whole. A thread that remembers "this range is mapped all through" can check with
/// it, without the mapping lock, that the answer still holds: nothing removed since overlaps
/// the range. And it can tell the same of a range it knows nothing of, when that lies inside
/// one that was just mapped. Without the log every change to the mappings voided every
/// remembered answer, and a title that commits and decommits pool memory several times a frame
/// sent the GPU thread back to the lock for each vertex buffer, to wait there for whichever
/// thread was mapping.
class MemoryRemovalLog final {
public:
    /// By the thread that holds the mapping lock for writing, before it unmaps anything.
    void Note(std::uint64_t address, std::uint64_t size) {
        Write(address, size & ~Added);
    }

    /// By the thread that holds the mapping lock for writing, once all of the range is mapped.
    void NoteAdded(std::uint64_t address, std::uint64_t size) {
        Write(address, size | Added);
    }

    /// Whether the range lies inside one that was mapped a moment ago, with nothing removed
    /// from it since. Dates the answer in `seen` when so.
    [[nodiscard]] bool JustMapped(std::uint64_t address, std::uint64_t size,
                                  std::uint64_t& seen) const {
        const std::uint64_t now = Count();
        const std::uint64_t oldest = now > Recent ? now - Recent : 0;
        const std::uint64_t end = address + size;
        for (std::uint64_t i = now; i-- != oldest;) {
            const Entry& entry = entries[i % entries.size()];
            const std::uint64_t logged = entry.address.load(std::memory_order_relaxed);
            const std::uint64_t raw_size = entry.size.load(std::memory_order_relaxed);
            const std::uint64_t logged_size = raw_size & ~Added;
            const std::uint64_t logged_end =
                logged_size > ~logged ? ~std::uint64_t{0} : logged + logged_size;
            if (!(logged < end && address < logged_end)) {
                continue;
            }
            // The newest entry that touches the range decides.
            if ((raw_size & Added) == 0 || logged > address || end > logged_end) {
                return false;
            }
            // (Unless the log went round meanwhile and this slot was being written again.)
            if (Count() - i >= entries.size()) {
                return false;
            }
            seen = now;
            return true;
        }
        return false;
    }

    /// For changes whose range is not worth working out: everything remembered is void.
    void NoteAll() {
        Note(0, ~Added);
    }

    /// How many ranges were logged so far. Read under the mapping lock, it dates an answer.
    [[nodiscard]] std::uint64_t Count() const {
        return count.load(std::memory_order_acquire);
    }

    /// Whether nothing logged since `seen` overlaps the range. Brings `seen` up to date when so.
    [[nodiscard]] bool Untouched(std::uint64_t address, std::uint64_t size,
                                 std::uint64_t& seen) const {
        const std::uint64_t now = Count();
        if (now == seen) {
            return true;
        }
        if (now - seen > entries.size()) {
            return false;
        }
        const std::uint64_t end = address + size;
        for (std::uint64_t i = seen; i != now; ++i) {
            const Entry& entry = entries[i % entries.size()];
            const std::uint64_t removed = entry.address.load(std::memory_order_relaxed);
            const std::uint64_t removed_size = entry.size.load(std::memory_order_relaxed);
            if ((removed_size & Added) != 0) {
                continue;
            }
            const std::uint64_t removed_end =
                removed_size > ~removed ? ~std::uint64_t{0} : removed + removed_size;
            if (removed < end && address < removed_end) {
                return false;
            }
        }
        // Entries read above may have been written over meanwhile, if the log went round: the
        // slot of the oldest one is being written as soon as the count is a whole round ahead.
        if (Count() - seen >= entries.size()) {
            return false;
        }
        seen = now;
        return true;
    }

private:
    /// The top bit of a logged size: the range was mapped, not removed.
    static constexpr std::uint64_t Added = std::uint64_t{1} << 63;
    /// How far back JustMapped looks.
    static constexpr std::uint64_t Recent = 96;

    void Write(std::uint64_t address, std::uint64_t size) {
        const std::uint64_t n = count.load(std::memory_order_relaxed);
        Entry& entry = entries[n % entries.size()];
        entry.address.store(address, std::memory_order_relaxed);
        entry.size.store(size, std::memory_order_relaxed);
        count.store(n + 1, std::memory_order_release);
    }

    struct Entry {
        std::atomic_uint64_t address{};
        std::atomic_uint64_t size{};
    };
    std::array<Entry, 1024> entries{};
    std::atomic_uint64_t count{};
};

} // namespace Core
