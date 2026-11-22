// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <httplib.h>

#include <algorithm>
#include <atomic>
#include <cctype>
#include <cfloat>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <thread>
#include <unordered_map>
#include <vector>

#include <SDL3/SDL_audio.h>
#include <SDL3/SDL_init.h>
#include <fmt/format.h>
#include <imgui.h>
#include <zlib.h>

#define STBTT_STATIC
#define STB_TRUETYPE_IMPLEMENTATION
#include <imstb_truetype.h>

#include "common/logging/log.h"
#include "common/stb.h"
#include "core/emulator_settings.h"
#include "imgui/imgui_texture.h"
#include "imgui/invite_banner.h"
#ifdef ENABLE_OPENXR_HOST
#include "core/vr/openxr_host.h"
#endif

namespace Libraries::Http {
bool ApplyHostOverride(std::string& scheme, std::string& host, u16& port, bool& is_secure);
}

namespace ImGui::InviteBanner {

namespace {

using Clock = std::chrono::steady_clock;

// Provisional wording, all of it.
struct Strings {
    const char* invited = "invited you";
    const char* accept_keys = "Hold Share + Cross";
    const char* accept = "Accept";
    const char* decline_keys = "Hold Share + Circle";
    const char* decline = "Decline";
    const char* xr_accept_keys = "Menu, then A";
    const char* xr_decline_keys = "Menu, then B";
    const char* xr_accept_keys_focused = "A";
    const char* xr_decline_keys_focused = "B";
    const char* joining_before = "Joining ";
    const char* joining_after = "...";
    const char* declined = "Invitation declined";
};
constexpr Strings Text{};

constexpr float SlideInSecs = 0.34f;
constexpr float VisibleSecs = 10.0f;
constexpr float SlideOutSecs = 0.26f;
constexpr float ResultSecs = 1.4f;
// How far above its place the panel starts and ends, in panel heights.
constexpr float SlideInFrom = 1.4f;
constexpr float SlideOutTo = 0.6f;

constexpr const char* AvatarHost = "fzh.avatar";

struct Rgba {
    float r, g, b, a;
};
constexpr Rgba Hex(u32 rgb, float a = 1.0f) {
    return {((rgb >> 16) & 0xff) / 255.0f, ((rgb >> 8) & 0xff) / 255.0f, (rgb & 0xff) / 255.0f, a};
}
constexpr Rgba ColPanel = Hex(0x0e1319, 0.86f);
constexpr Rgba ColLine = Hex(0xa0dce1, 0.28f);
constexpr Rgba ColFg = Hex(0xeef4f6);
constexpr Rgba ColDim = Hex(0x9fb0b8);
constexpr Rgba ColAccent = Hex(0x37d3bd);
constexpr Rgba ColOk = Hex(0x6ee39a);
constexpr Rgba ColNo = Hex(0xff6b6b);
constexpr Rgba ColKey = Hex(0xffffff, 0.10f);
constexpr Rgba ColInk = Hex(0x062a26);
constexpr Rgba ColTile = Hex(0x5adcca);

// The panel's layout in its own units: 640 wide, scaled to wherever it is drawn.
constexpr float PanelW = 640.0f;
constexpr float PanelH = 98.0f;
constexpr float Radius = 11.0f;
constexpr float PadX = 18.0f;
constexpr float PadTop = 15.0f;
constexpr float AvatarSize = 64.0f;
constexpr float BarH = 4.5f;

struct Box {
    float x, y, w, h;
};

enum class Corners { All, Bottom, BottomLeft };

class Canvas {
public:
    virtual ~Canvas() = default;
    virtual void Fill(Box r, float radius, Rgba c, Corners corners = Corners::All) = 0;
    virtual void Stroke(Box r, float radius, float width, Rgba c) = 0;
    virtual bool HasPicture() const = 0;
    virtual void Picture(Box r, float radius) = 0;
    virtual float Measure(float size, std::string_view text) = 0;
    virtual void Write(float x, float y, float size, bool bold, Rgba c, std::string_view text) = 0;
};

struct Content {
    std::string name;
    std::string initials;
    Answer result = Answer::None;
    bool focused = false;
    bool xr_hints = false;
};

struct Hits {
    Box accept{};
    Box decline{};
};

Hits PaintBody(Canvas& c, const Content& k) {
    Hits hits;
    c.Fill({0, 0, PanelW, PanelH}, Radius, ColPanel);
    c.Stroke({0, 0, PanelW, PanelH}, Radius, k.focused ? 1.5f : 1.0f,
             k.focused ? Rgba{ColAccent.r, ColAccent.g, ColAccent.b, 0.9f} : ColLine);

    const Box tile{PadX, PadTop, AvatarSize, AvatarSize};
    if (c.HasPicture()) {
        c.Picture(tile, 8.0f);
    } else {
        c.Fill(tile, 8.0f, ColTile);
        constexpr float size = 29.0f;
        const float w = c.Measure(size, k.initials);
        c.Write(tile.x + (tile.w - w) / 2.0f, tile.y + (tile.h - size) / 2.0f, size, true, ColInk,
                k.initials);
    }

    const float text_x = PadX + AvatarSize + 18.0f;
    if (k.result != Answer::None) {
        constexpr float size = 26.0f;
        const std::string line = k.result == Answer::Accept
                                     ? Text.joining_before + k.name + Text.joining_after
                                     : std::string{Text.declined};
        c.Write(text_x, PadTop + (AvatarSize - size) / 2.0f, size, true,
                k.result == Answer::Accept ? ColOk : ColNo, line);
        return hits;
    }

    constexpr float key_size = 15.5f;
    constexpr float chip_pad = 8.0f;
    constexpr float chip_h = 26.0f;
    constexpr float row_gap = 7.0f;
    const float right = PanelW - PadX;
    const float row_top = PadTop + (AvatarSize - (2 * chip_h + row_gap)) / 2.0f;
    const auto row_width = [&](const char* keys, const char* label) {
        return c.Measure(key_size, keys) + 2 * chip_pad + 7.0f + c.Measure(key_size, label);
    };
    const char* accept_keys = !k.xr_hints ? Text.accept_keys
                              : k.focused ? Text.xr_accept_keys_focused
                                          : Text.xr_accept_keys;
    const char* decline_keys = !k.xr_hints ? Text.decline_keys
                               : k.focused ? Text.xr_decline_keys_focused
                                           : Text.xr_decline_keys;
    const float keys_w = std::max(row_width(accept_keys, Text.accept),
                                  row_width(decline_keys, Text.decline));
    const auto row = [&](float y, const char* keys, const char* label) {
        const float label_w = c.Measure(key_size, label);
        const float chip_w = c.Measure(key_size, keys) + 2 * chip_pad;
        const float chip_x = right - label_w - 7.0f - chip_w;
        c.Fill({chip_x, y, chip_w, chip_h}, 5.0f, ColKey);
        c.Write(chip_x + chip_pad, y + (chip_h - key_size) / 2.0f, key_size, false, ColFg, keys);
        c.Write(right - label_w, y + (chip_h - key_size) / 2.0f, key_size, false, ColDim, label);
        return Box{right - keys_w, y, keys_w, chip_h};
    };
    hits.accept = row(row_top, accept_keys, Text.accept);
    hits.decline = row(row_top + chip_h + row_gap, decline_keys, Text.decline);

    constexpr float name_size = 30.0f;
    constexpr float what_size = 17.5f;
    const float max_w = right - keys_w - 18.0f - text_x;
    std::string name = k.name;
    if (c.Measure(name_size, name) > max_w) {
        while (!name.empty() && c.Measure(name_size, name + "...") > max_w) {
            name.pop_back();
            while (!name.empty() && (static_cast<u8>(name.back()) & 0xc0) == 0x80) {
                name.pop_back();
            }
        }
        name += "...";
    }
    c.Write(text_x, PadTop + 4.0f, name_size, true, ColFg, name);
    c.Write(text_x, PadTop + 41.0f, what_size, false, ColDim, Text.invited);
    return hits;
}

void PaintBar(Canvas& c, float bar) {
    const Box track{0, PanelH - BarH, PanelW, BarH};
    c.Fill(track, Radius, ColKey, Corners::Bottom);
    if (bar > 0.0f) {
        c.Fill({0, track.y, PanelW * bar, BarH}, Radius, ColAccent, Corners::BottomLeft);
    }
}

u32 NextCodepoint(std::string_view s, std::size_t& i) {
    const u8 c = static_cast<u8>(s[i++]);
    if (c < 0x80) {
        return c;
    }
    int extra = c >= 0xf0 ? 3 : c >= 0xe0 ? 2 : c >= 0xc0 ? 1 : 0;
    u32 cp = c & (0x3f >> extra);
    while (extra-- > 0 && i < s.size()) {
        cp = (cp << 6) | (static_cast<u8>(s[i++]) & 0x3f);
    }
    return cp;
}

enum class Phase { Hidden, In, Live, Result, Out };

struct State {
    Phase phase = Phase::Hidden;
    Clock::time_point since;
    std::string key;
    std::string online_id;
    Content content;
    // The line while sliding out: where it stood when the banner began to leave, or below
    // zero when it is not shown (after an outcome).
    float out_bar = -1.0f;
    // While focused the countdown stands still: since when (empty while it runs).
    Clock::time_point paused_at{};
    u32 version = 1;
    u32 shows = 0;
};

std::mutex g_mutex;
State g;

float Seconds(Clock::time_point from, Clock::time_point to) {
    return std::chrono::duration<float>(to - from).count();
}

bool Paused() {
    return g.phase == Phase::Live && g.paused_at != Clock::time_point{};
}

void Pause(Clock::time_point at) {
    g.paused_at = at;
    LOG_INFO(ImGui, "Invitation banner: focused, the countdown stops");
}

void Advance(Clock::time_point now) {
    for (;;) {
        if (Paused()) {
            return;
        }
        const float t = Seconds(g.since, now);
        const auto next = [&](Phase phase, float after) {
            g.phase = phase;
            g.since += std::chrono::duration_cast<Clock::duration>(
                std::chrono::duration<float>(after));
        };
        if (g.phase == Phase::In && t >= SlideInSecs) {
            next(Phase::Live, SlideInSecs);
            if (g.content.focused) {
                Pause(g.since);
            }
        } else if (g.phase == Phase::Live && t >= VisibleSecs) {
            g.out_bar = 0.0f;
            next(Phase::Out, VisibleSecs);
            LOG_INFO(ImGui, "Invitation banner: unanswered, leaving (still answerable by holding "
                            "Share, or Menu twice on the headset)");
        } else if (g.phase == Phase::Result && t >= ResultSecs) {
            g.out_bar = -1.0f;
            next(Phase::Out, ResultSecs);
        } else if (g.phase == Phase::Out && t >= SlideOutSecs) {
            g.phase = Phase::Hidden;
            ++g.version;
            LOG_INFO(ImGui, "Invitation banner: gone");
            return;
        } else {
            return;
        }
    }
}

// CSS cubic-bezier(x1, y1, x2, y2) at x.
float Bezier(float x1, float y1, float x2, float y2, float x) {
    const auto at = [](float a, float b, float t) {
        const float u = 1.0f - t;
        return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t;
    };
    float lo = 0.0f, hi = 1.0f, t = x;
    for (int i = 0; i < 24; ++i) {
        t = (lo + hi) / 2.0f;
        (at(x1, x2, t) < x ? lo : hi) = t;
    }
    return at(y1, y2, t);
}

struct View {
    bool visible = false;
    float lift = 0.0f;
    float opacity = 1.0f;
    float bar = -1.0f;
};

View ViewAt(Clock::time_point now) {
    View v;
    const float t = Seconds(g.since, now);
    switch (g.phase) {
    case Phase::Hidden:
        return v;
    case Phase::In: {
        const float e = Bezier(0.2f, 0.8f, 0.2f, 1.0f, std::clamp(t / SlideInSecs, 0.0f, 1.0f));
        v.lift = SlideInFrom * (1.0f - e);
        v.opacity = e;
        v.bar = 1.0f;
        break;
    }
    case Phase::Live: {
        const float shown = Paused() ? Seconds(g.since, g.paused_at) : t;
        v.bar = std::clamp(1.0f - shown / VisibleSecs, 0.0f, 1.0f);
        break;
    }
    case Phase::Result:
        break;
    case Phase::Out: {
        const float e = Bezier(0.42f, 0.0f, 1.0f, 1.0f, std::clamp(t / SlideOutSecs, 0.0f, 1.0f));
        v.lift = SlideOutTo * e;
        v.opacity = 1.0f - e;
        v.bar = g.out_bar;
        break;
    }
    }
    v.visible = true;
    return v;
}

struct Avatar {
    enum class Status { Loading, Ready, Failed };
    Status status = Status::Loading;
    std::vector<u8> png;
    std::vector<u8> rgba;
    int width = 0;
    int height = 0;
    RefCountedTexture texture;
    bool texture_made = false;
};

std::mutex g_avatar_mutex;
std::unordered_map<std::string, std::shared_ptr<Avatar>> g_avatars;

std::shared_ptr<Avatar> FindAvatar(const std::string& online_id) {
    std::scoped_lock lock{g_avatar_mutex};
    const auto it = g_avatars.find(online_id);
    return it != g_avatars.end() ? it->second : nullptr;
}

void RequestAvatar(const std::string& online_id) {
    if (online_id.empty()) {
        return;
    }
    {
        std::scoped_lock lock{g_avatar_mutex};
        auto& slot = g_avatars[online_id];
        if (slot && slot->status != Avatar::Status::Failed) {
            return;
        }
        slot = std::make_shared<Avatar>();
    }
    std::thread([online_id] {
        std::string scheme = "http";
        std::string host = AvatarHost;
        u16 port = 80;
        bool secure = false;
        Libraries::Http::ApplyHostOverride(scheme, host, port, secure);
        std::string path = "/a/";
        for (const char ch : online_id) {
            if (std::isalnum(static_cast<u8>(ch)) || ch == '-' || ch == '_' || ch == '.') {
                path += ch;
            } else {
                path += fmt::format("%{:02X}", static_cast<u8>(ch));
            }
        }
        path += ".png";
        const std::string base = fmt::format("{}://{}:{}", scheme, host, port);
        httplib::Client client(base);
        client.set_connection_timeout(3);
        client.set_read_timeout(5);
        const auto res = client.Get(path);

        auto avatar = std::make_shared<Avatar>();
        avatar->status = Avatar::Status::Failed;
        // The backend's stand-in for an account that never uploaded one: initials look better.
        const bool placeholder = res && res->get_header_value("X-Avatar-Placeholder") == "1";
        if (res && res->status == 200 && !res->body.empty() && !placeholder) {
            int w = 0, h = 0, channels = 0;
            stbi_uc* pixels =
                stbi_load_from_memory(reinterpret_cast<const stbi_uc*>(res->body.data()),
                                      static_cast<int>(res->body.size()), &w, &h, &channels, 4);
            if (pixels != nullptr) {
                avatar->rgba.assign(pixels, pixels + static_cast<std::size_t>(w) * h * 4);
                avatar->width = w;
                avatar->height = h;
                avatar->png.assign(res->body.begin(), res->body.end());
                avatar->status = Avatar::Status::Ready;
                stbi_image_free(pixels);
            }
        }
        if (avatar->status == Avatar::Status::Ready) {
            LOG_INFO(ImGui, "Invitation banner: avatar of {} from {}{} ({}x{})", online_id, base,
                     path, avatar->width, avatar->height);
        } else {
            LOG_INFO(ImGui, "Invitation banner: no avatar of {} from {}{} ({}): initials instead",
                     online_id, base, path,
                     placeholder ? std::string{"a placeholder, none uploaded"}
                     : res       ? fmt::format("HTTP {}", res->status)
                                 : httplib::to_string(res.error()));
        }
        {
            std::scoped_lock lock{g_avatar_mutex};
            g_avatars[online_id] = avatar;
        }
        std::scoped_lock lock{g_mutex};
        ++g.version;
    }).detach();
}

constexpr int ChimeRate = 48000;

std::vector<float> ChimeSamples() {
    struct Note {
        float freq, at, length;
    };
    // "Rising three": C5, E5, G5.
    constexpr Note notes[] = {{523.25f, 0.0f, 0.22f}, {659.25f, 0.09f, 0.22f},
                              {783.99f, 0.18f, 0.5f}};
    constexpr float peak = 0.13f;
    constexpr float attack = 0.015f;
    constexpr float floor_level = 0.0001f;
    std::vector<float> out(static_cast<std::size_t>((0.18f + 0.5f + 0.05f) * ChimeRate), 0.0f);
    for (const Note& n : notes) {
        const auto first = static_cast<std::size_t>(n.at * ChimeRate);
        const auto count = static_cast<std::size_t>(n.length * ChimeRate);
        for (std::size_t i = 0; i < count && first + i < out.size(); ++i) {
            const float t = static_cast<float>(i) / ChimeRate;
            const float env =
                t < attack ? floor_level * std::pow(peak / floor_level, t / attack)
                           : peak * std::pow(floor_level / peak, (t - attack) / (n.length - attack));
            out[first + i] += env * std::sin(6.2831853f * n.freq * t);
        }
    }
    return out;
}

// The device the game's sound goes to (see SDLPortBackend::GetDeviceName), looked up by name.
SDL_AudioDeviceID ChimeDevice(std::string& name) {
    name = EmulatorSettings.GetAudioBackend() == 0 ? EmulatorSettings.GetSDLMainOutputDevice()
                                                   : std::string{};
    if (name == "None") {
        return 0;
    }
#ifdef ENABLE_OPENXR_HOST
    if (name.empty() || name == "Default Device") {
        if (std::string headset = ::Core::Vr::OpenXrHost::Instance().AudioOutputName();
            !headset.empty()) {
            name = std::move(headset);
        }
    }
#endif
    if (name.empty() || name == "Default Device") {
        const char* system = SDL_GetAudioDeviceName(SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK);
        name = system != nullptr ? system : "";
    }
    int count = 0;
    SDL_AudioDeviceID* devices = SDL_GetAudioPlaybackDevices(&count);
    SDL_AudioDeviceID found = 0;
    for (int i = 0; devices != nullptr && i < count; ++i) {
        const char* dev_name = SDL_GetAudioDeviceName(devices[i]);
        if (dev_name != nullptr && name == dev_name) {
            found = devices[i];
            break;
        }
    }
    if (found == 0 && devices != nullptr && count > 0) {
        found = SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK;
    }
    SDL_free(devices);
    return found;
}

void WriteWav(const std::filesystem::path& path, const std::vector<float>& samples) {
    std::vector<s16> pcm(samples.size());
    for (std::size_t i = 0; i < samples.size(); ++i) {
        pcm[i] = static_cast<s16>(std::clamp(samples[i], -1.0f, 1.0f) * 32767.0f);
    }
    const u32 data_bytes = static_cast<u32>(pcm.size() * 2);
    std::ofstream f(path, std::ios::binary);
    const auto u32le = [&](u32 v) { f.write(reinterpret_cast<const char*>(&v), 4); };
    const auto u16le = [&](u16 v) { f.write(reinterpret_cast<const char*>(&v), 2); };
    f.write("RIFF", 4);
    u32le(36 + data_bytes);
    f.write("WAVEfmt ", 8);
    u32le(16);
    u16le(1);
    u16le(1);
    u32le(ChimeRate);
    u32le(ChimeRate * 2);
    u16le(2);
    u16le(16);
    f.write("data", 4);
    u32le(data_bytes);
    f.write(reinterpret_cast<const char*>(pcm.data()), data_bytes);
}

const char* DumpDir() {
    static const char* dir = [] {
        const char* value = std::getenv("SHADPS4_DBG_INVITE_DUMP");
        return value != nullptr && value[0] != '\0' ? value : nullptr;
    }();
    return dir;
}

// On a stream of its own, so the game's audio ports are left as they are.
void PlayChime() {
    std::thread([] {
        const std::vector<float> samples = ChimeSamples();
        if (DumpDir() != nullptr) {
            WriteWav(std::filesystem::path{DumpDir()} / "invite_chime.wav", samples);
        }
        if (SDL_WasInit(SDL_INIT_AUDIO) == 0 && !SDL_InitSubSystem(SDL_INIT_AUDIO)) {
            LOG_WARNING(ImGui, "Invitation chime: no audio ({})", SDL_GetError());
            return;
        }
        std::string name;
        const SDL_AudioDeviceID device = ChimeDevice(name);
        if (device == 0) {
            LOG_INFO(ImGui, "Invitation chime: no output device ('{}')", name);
            return;
        }
        const SDL_AudioSpec spec{SDL_AUDIO_F32, 1, ChimeRate};
        SDL_AudioStream* stream = SDL_OpenAudioDeviceStream(device, &spec, nullptr, nullptr);
        if (stream == nullptr) {
            LOG_WARNING(ImGui, "Invitation chime: cannot open '{}' ({})", name, SDL_GetError());
            return;
        }
        const int bytes = static_cast<int>(samples.size() * sizeof(float));
        const bool queued = SDL_PutAudioStreamData(stream, samples.data(), bytes) &&
                            SDL_FlushAudioStream(stream) && SDL_ResumeAudioStreamDevice(stream);
        float peak = 0.0f;
        for (const float s : samples) {
            peak = std::max(peak, std::abs(s));
        }
        LOG_INFO(ImGui, "Invitation chime: {} samples ({:.2f} s at {} Hz, peak {:.3f}) {} on '{}'",
                 samples.size(), static_cast<float>(samples.size()) / ChimeRate, ChimeRate, peak,
                 queued ? "queued" : "NOT queued", name);
        const auto deadline = Clock::now() + std::chrono::seconds{3};
        while (queued && SDL_GetAudioStreamQueued(stream) > 0 && Clock::now() < deadline) {
            std::this_thread::sleep_for(std::chrono::milliseconds{50});
        }
        const int left = SDL_GetAudioStreamQueued(stream);
        // What is past the stream may still be in the device's own buffer.
        std::this_thread::sleep_for(std::chrono::milliseconds{150});
        SDL_DestroyAudioStream(stream);
        LOG_INFO(ImGui, "Invitation chime: consumed by the device, {} bytes left", left);
    }).detach();
}

class ImGuiCanvas final : public Canvas {
public:
    ImGuiCanvas(ImDrawList* list, ImVec2 origin, float scale, float alpha, ImFont* font,
                ImTextureID picture)
        : list{list}, origin{origin}, s{scale}, alpha{alpha}, font{font}, picture{picture} {}

    void Fill(Box r, float radius, Rgba c, Corners corners) override {
        const ImDrawFlags flags = corners == Corners::Bottom       ? ImDrawFlags_RoundCornersBottom
                                  : corners == Corners::BottomLeft ? ImDrawFlags_RoundCornersBottomLeft
                                                                   : ImDrawFlags_RoundCornersAll;
        list->AddRectFilled(Min(r), Max(r), Col(c), radius * s, flags);
    }
    void Stroke(Box r, float radius, float width, Rgba c) override {
        list->AddRect(Min(r), Max(r), Col(c), radius * s, 0, std::max(1.0f, width * s));
    }
    bool HasPicture() const override {
        return picture != ImTextureID{};
    }
    void Picture(Box r, float radius) override {
        list->AddImageRounded(picture, Min(r), Max(r), {0, 0}, {1, 1}, Col({1, 1, 1, 1}),
                              radius * s);
    }
    float Measure(float size, std::string_view text) override {
        return font->CalcTextSizeA(size * s, FLT_MAX, 0.0f, text.data(), text.data() + text.size())
                   .x /
               s;
    }
    void Write(float x, float y, float size, bool bold, Rgba c, std::string_view text) override {
        const ImVec2 at{origin.x + x * s, origin.y + y * s};
        list->AddText(font, size * s, at, Col(c), text.data(), text.data() + text.size());
        if (bold) {
            list->AddText(font, size * s, {at.x + std::max(1.0f, 0.03f * size * s), at.y}, Col(c),
                          text.data(), text.data() + text.size());
        }
    }

    ImVec2 Min(Box r) const {
        return {origin.x + r.x * s, origin.y + r.y * s};
    }
    ImVec2 Max(Box r) const {
        return {origin.x + (r.x + r.w) * s, origin.y + (r.y + r.h) * s};
    }

private:
    ImU32 Col(Rgba c) const {
        return ImGui::ColorConvertFloat4ToU32({c.r, c.g, c.b, c.a * alpha});
    }

    ImDrawList* list;
    ImVec2 origin;
    float s;
    float alpha;
    ImFont* font;
    ImTextureID picture;
};

struct TtfFont {
    std::vector<u8> data;
    stbtt_fontinfo info{};
    float ascent = 0.0f;
    bool ok = false;
};
TtfFont g_font;
bool g_font_tried = false;

// The interface font's own TTF, as the ImGui atlas holds it (it keeps the data for glyphs it
// has yet to bake). Called on the ImGui thread.
void CaptureFont() {
    if (g_font_tried) {
        return;
    }
    g_font_tried = true;
    const ImFontAtlas* atlas = ImGui::GetIO().Fonts;
    for (const ImFontConfig& source : atlas->Sources) {
        if (source.MergeMode || source.FontData == nullptr || source.FontDataSize <= 0) {
            continue;
        }
        TtfFont font;
        const auto* bytes = static_cast<const u8*>(source.FontData);
        font.data.assign(bytes, bytes + source.FontDataSize);
        if (stbtt_InitFont(&font.info, font.data.data(),
                           stbtt_GetFontOffsetForIndex(font.data.data(), 0)) != 0) {
            int ascent = 0, descent = 0, gap = 0;
            stbtt_GetFontVMetrics(&font.info, &ascent, &descent, &gap);
            font.ascent = static_cast<float>(ascent);
            font.ok = true;
            std::scoped_lock lock{g_mutex};
            g_font = std::move(font);
            // stbtt_fontinfo points into `data`, which moved with it.
            stbtt_InitFont(&g_font.info, g_font.data.data(),
                           stbtt_GetFontOffsetForIndex(g_font.data.data(), 0));
            ++g.version;
        }
        break;
    }
    if (!g_font.ok) {
        LOG_WARNING(ImGui, "Invitation banner: no font for the headset's panel, text left out");
    }
}

float Coverage(float signed_distance) {
    return std::clamp(0.5f - signed_distance, 0.0f, 1.0f);
}

float RoundedDistance(float px, float py, float x, float y, float w, float h, float r) {
    r = std::min(r, std::min(w, h) / 2.0f);
    const float qx = std::abs(px - (x + w / 2.0f)) - (w / 2.0f - r);
    const float qy = std::abs(py - (y + h / 2.0f)) - (h / 2.0f - r);
    const float ox = std::max(qx, 0.0f);
    const float oy = std::max(qy, 0.0f);
    return std::sqrt(ox * ox + oy * oy) + std::min(std::max(qx, qy), 0.0f) - r;
}

// Straight alpha, sRGB, floats; clipped to the panel's rounded outline as CSS overflow is.
class PixelCanvas final : public Canvas {
public:
    PixelCanvas(float* pixels, int width, int height, const Avatar* avatar)
        : px{pixels}, w{width}, h{height}, s{static_cast<float>(width) / PanelW}, avatar{avatar} {}

    void Fill(Box r, float radius, Rgba c, Corners) override {
        Each(r, [&](int x, int y, float fx, float fy) {
            Blend(x, y, c, c.a * Coverage(Distance(fx, fy, r, radius)));
        });
    }
    void Stroke(Box r, float radius, float width, Rgba c) override {
        Each(r, [&](int x, int y, float fx, float fy) {
            const float d = Distance(fx, fy, r, radius);
            Blend(x, y, c, c.a * (Coverage(d) - Coverage(d + width * s)));
        });
    }
    bool HasPicture() const override {
        return avatar != nullptr;
    }
    void Picture(Box r, float radius) override {
        const float step_x = avatar->width / (r.w * s);
        const float step_y = avatar->height / (r.h * s);
        Each(r, [&](int x, int y, float fx, float fy) {
            const float cov = Coverage(Distance(fx, fy, r, radius));
            if (cov <= 0.0f) {
                return;
            }
            const int u0 = std::clamp(static_cast<int>((x - r.x * s) * step_x), 0, avatar->width - 1);
            const int v0 = std::clamp(static_cast<int>((y - r.y * s) * step_y), 0, avatar->height - 1);
            const int u1 = std::clamp(static_cast<int>((x + 1 - r.x * s) * step_x), u0 + 1, avatar->width);
            const int v1 = std::clamp(static_cast<int>((y + 1 - r.y * s) * step_y), v0 + 1, avatar->height);
            float sum[4]{};
            for (int v = v0; v < v1; ++v) {
                for (int u = u0; u < u1; ++u) {
                    const u8* p = &avatar->rgba[(static_cast<std::size_t>(v) * avatar->width + u) * 4];
                    for (int ch = 0; ch < 4; ++ch) {
                        sum[ch] += p[ch];
                    }
                }
            }
            const float n = static_cast<float>((u1 - u0) * (v1 - v0)) * 255.0f;
            Blend(x, y, {sum[0] / n, sum[1] / n, sum[2] / n, 1.0f}, sum[3] / n * cov);
        });
    }
    float Measure(float size, std::string_view text) override {
        if (!g_font.ok) {
            return size * 0.55f * static_cast<float>(text.size());
        }
        const float scale = stbtt_ScaleForPixelHeight(&g_font.info, size * s);
        float pen = 0.0f;
        for (std::size_t i = 0; i < text.size();) {
            const u32 cp = NextCodepoint(text, i);
            int advance = 0, bearing = 0;
            stbtt_GetCodepointHMetrics(&g_font.info, static_cast<int>(cp), &advance, &bearing);
            pen += advance * scale;
            if (i < text.size()) {
                std::size_t j = i;
                pen += stbtt_GetCodepointKernAdvance(&g_font.info, static_cast<int>(cp),
                                                     static_cast<int>(NextCodepoint(text, j))) *
                       scale;
            }
        }
        return pen / s;
    }
    void Write(float x, float y, float size, bool bold, Rgba c, std::string_view text) override {
        if (!g_font.ok) {
            return;
        }
        const float scale = stbtt_ScaleForPixelHeight(&g_font.info, size * s);
        const int baseline = static_cast<int>(std::lround(y * s + g_font.ascent * scale));
        const int thicken = bold ? 1 : 0;
        float pen = x * s;
        std::vector<u8> glyph;
        for (std::size_t i = 0; i < text.size();) {
            const u32 cp = NextCodepoint(text, i);
            const float shift = pen - std::floor(pen);
            int x0 = 0, y0 = 0, x1 = 0, y1 = 0;
            stbtt_GetCodepointBitmapBoxSubpixel(&g_font.info, static_cast<int>(cp), scale, scale,
                                                shift, 0.0f, &x0, &y0, &x1, &y1);
            const int gw = x1 - x0;
            const int gh = y1 - y0;
            if (gw > 0 && gh > 0) {
                glyph.assign(static_cast<std::size_t>(gw) * gh, 0);
                stbtt_MakeCodepointBitmapSubpixel(&g_font.info, glyph.data(), gw, gh, gw, scale,
                                                  scale, shift, 0.0f, static_cast<int>(cp));
                const int ox = static_cast<int>(std::floor(pen)) + x0;
                const int oy = baseline + y0;
                for (int gy = 0; gy < gh; ++gy) {
                    for (int gx = 0; gx < gw + thicken; ++gx) {
                        u8 v = gx < gw ? glyph[gy * gw + gx] : 0;
                        if (thicken != 0 && gx > 0) {
                            v = std::max(v, glyph[gy * gw + gx - 1]);
                        }
                        if (v != 0) {
                            Blend(ox + gx, oy + gy, c, c.a * v / 255.0f);
                        }
                    }
                }
            }
            int advance = 0, bearing = 0;
            stbtt_GetCodepointHMetrics(&g_font.info, static_cast<int>(cp), &advance, &bearing);
            pen += advance * scale;
            if (i < text.size()) {
                std::size_t j = i;
                pen += stbtt_GetCodepointKernAdvance(&g_font.info, static_cast<int>(cp),
                                                     static_cast<int>(NextCodepoint(text, j))) *
                       scale;
            }
        }
    }

private:
    template <typename F>
    void Each(Box r, F&& f) {
        const int x0 = std::max(0, static_cast<int>(std::floor(r.x * s)) - 1);
        const int y0 = std::max(0, static_cast<int>(std::floor(r.y * s)) - 1);
        const int x1 = std::min(w, static_cast<int>(std::ceil((r.x + r.w) * s)) + 1);
        const int y1 = std::min(h, static_cast<int>(std::ceil((r.y + r.h) * s)) + 1);
        for (int y = y0; y < y1; ++y) {
            for (int x = x0; x < x1; ++x) {
                f(x, y, x + 0.5f, y + 0.5f);
            }
        }
    }
    float Distance(float fx, float fy, Box r, float radius) const {
        return RoundedDistance(fx, fy, r.x * s, r.y * s, r.w * s, r.h * s, radius * s);
    }
    float Clip(float fx, float fy) const {
        return Coverage(RoundedDistance(fx, fy, 0, 0, PanelW * s, PanelH * s, Radius * s));
    }
    void Blend(int x, int y, Rgba c, float a) {
        if (x < 0 || y < 0 || x >= w || y >= h) {
            return;
        }
        a *= Clip(x + 0.5f, y + 0.5f);
        if (a <= 0.0f) {
            return;
        }
        float* p = &px[(static_cast<std::size_t>(y) * w + x) * 4];
        const float keep = p[3] * (1.0f - a);
        const float out = a + keep;
        p[0] = (c.r * a + p[0] * keep) / out;
        p[1] = (c.g * a + p[1] * keep) / out;
        p[2] = (c.b * a + p[2] * keep) / out;
        p[3] = out;
    }

    float* px;
    int w;
    int h;
    float s;
    const Avatar* avatar;
};

struct QuadCache {
    u32 version = 0;
    u32 requested = 0;
    u32 show = 0;
    std::vector<float> base;
    std::vector<float> work;
};
QuadCache g_quad;

struct Lut {
    float decode[256];
    u8 encode[4096];
    Lut() {
        for (int i = 0; i < 256; ++i) {
            const float c = i / 255.0f;
            decode[i] = c <= 0.04045f ? c / 12.92f : std::pow((c + 0.055f) / 1.055f, 2.4f);
        }
        for (int i = 0; i < 4096; ++i) {
            const float l = i / 4095.0f;
            const float c = l <= 0.0031308f ? l * 12.92f : 1.055f * std::pow(l, 1.0f / 2.4f) - 0.055f;
            encode[i] = static_cast<u8>(std::lround(std::clamp(c, 0.0f, 1.0f) * 255.0f));
        }
    }
};

// Drawing the panel takes a few milliseconds: done on a thread of its own, never on the
// headset's frame thread, which keeps showing the previous picture meanwhile.
void StartBaseRender() {
    g_quad.requested = g.version;
    std::thread([content = g.content, online_id = g.online_id, version = g.version,
                 show = g.shows] {
        const auto started = Clock::now();
        std::vector<float> base(std::size_t{QuadWidth} * QuadHeight * 4, 0.0f);
        const std::shared_ptr<Avatar> avatar = FindAvatar(online_id);
        const bool ready = avatar && avatar->status == Avatar::Status::Ready;
        PixelCanvas canvas(base.data(), QuadWidth, QuadHeight, ready ? avatar.get() : nullptr);
        PaintBody(canvas, content);
        const float took =
            std::chrono::duration<float, std::milli>(Clock::now() - started).count();
        std::scoped_lock lock{g_mutex};
        if (version > g_quad.version) {
            g_quad.base = std::move(base);
            g_quad.version = version;
            g_quad.show = show;
        }
        LOG_DEBUG(ImGui, "Invitation banner: headset picture redrawn in {:.2f} ms", took);
    }).detach();
}

void Convert(u8* out, bool bgra, u32 row0, float opacity) {
    static const Lut lut;
    const auto q = [](float v) { return static_cast<int>(std::clamp(v, 0.0f, 1.0f) * 255.0f + 0.5f); };
    for (u32 y = row0; y < QuadHeight; ++y) {
        for (u32 x = 0; x < QuadWidth; ++x) {
            const std::size_t i = (std::size_t{y} * QuadWidth + x) * 4;
            const float* p = &g_quad.work[i];
            const float a = p[3] * opacity;
            u8* o = &out[i];
            if (a <= 0.0f) {
                o[0] = o[1] = o[2] = o[3] = 0;
                continue;
            }
            const auto channel = [&](float v) {
                return lut.encode[static_cast<int>(lut.decode[q(v)] * a * 4095.0f + 0.5f)];
            };
            const u8 r = channel(p[0]);
            const u8 gc = channel(p[1]);
            const u8 b = channel(p[2]);
            o[0] = bgra ? b : r;
            o[1] = gc;
            o[2] = bgra ? r : b;
            o[3] = static_cast<u8>(q(a));
        }
    }
}

bool ComposeLocked(u8* pixels, bool bgra, bool fresh, u64& key, float& lift) {
    Advance(Clock::now());
    const View v = ViewAt(Clock::now());
    if (!v.visible) {
        return false;
    }
    if (g_quad.requested != g.version) {
        StartBaseRender();
    }
    if (g_quad.base.empty() || g_quad.show != g.shows) {
        // Nothing drawn yet for this invitation; not the previous one's panel either.
        return false;
    }
    lift = v.lift;
    const u64 opacity = static_cast<u64>(std::lround(v.opacity * 255.0f));
    const u64 bar = v.bar < 0.0f ? 0xffff : static_cast<u64>(std::lround(v.bar * QuadWidth));
    const u64 want = (u64{g_quad.version} << 32) | (opacity << 16) | bar;
    if (!fresh && want == key) {
        return true;
    }
    const bool full = fresh || (want >> 16) != (key >> 16);
    const float scale = static_cast<float>(QuadWidth) / PanelW;
    const u32 row0 = full ? 0
                          : static_cast<u32>(std::max(
                                0.0f, std::floor((PanelH - BarH) * scale) - 1.0f));
    g_quad.work.resize(g_quad.base.size());
    const std::size_t from = std::size_t{row0} * QuadWidth * 4;
    std::copy(g_quad.base.begin() + from, g_quad.base.end(), g_quad.work.begin() + from);
    if (v.bar >= 0.0f) {
        PixelCanvas canvas(g_quad.work.data(), QuadWidth, QuadHeight, nullptr);
        PaintBar(canvas, v.bar);
    }
    Convert(pixels, bgra, row0, v.opacity);
    key = want;
    return true;
}

// SHADPS4_DBG_INVITE_DUMP=<folder>: the headset's picture of the banner, over a dark
// background, once while it waits for an answer, once with the outcome and once as it leaves.
// False while the panel is still being drawn for the current state: try again later.
bool DumpQuad(const char* what, u32 show) {
    std::vector<u8> pixels(std::size_t{QuadWidth} * QuadHeight * 4);
    u64 key = 0;
    float lift = 0.0f;
    const auto started = Clock::now();
    {
        std::scoped_lock lock{g_mutex};
        if (!ComposeLocked(pixels.data(), false, true, key, lift)) {
            return false;
        }
        if (g_quad.version != g.version) {
            return false;
        }
    }
    LOG_INFO(ImGui, "Invitation banner: headset frame composed in {:.2f} ms",
             std::chrono::duration<float, std::milli>(Clock::now() - started).count());
    // PNG rows, each led by filter type 0.
    std::vector<u8> raw;
    raw.reserve(std::size_t{QuadWidth * 3 + 1} * QuadHeight);
    constexpr u8 backdrop[3] = {40, 52, 64};
    for (u32 y = 0; y < QuadHeight; ++y) {
        raw.push_back(0);
        for (u32 x = 0; x < QuadWidth; ++x) {
            const u8* p = &pixels[(std::size_t{y} * QuadWidth + x) * 4];
            for (int ch = 0; ch < 3; ++ch) {
                raw.push_back(static_cast<u8>(
                    std::min(255.0f, p[ch] + backdrop[ch] * (1.0f - p[3] / 255.0f))));
            }
        }
    }
    uLongf packed_size = compressBound(static_cast<uLong>(raw.size()));
    std::vector<u8> packed(packed_size);
    if (compress2(packed.data(), &packed_size, raw.data(), static_cast<uLong>(raw.size()), 6) !=
        Z_OK) {
        return true;
    }
    packed.resize(packed_size);
    std::string png = "\x89PNG\r\n\x1a\n";
    const auto chunk = [&](const char* type, const std::vector<u8>& data) {
        const auto be32 = [&](u32 v) {
            for (int shift = 24; shift >= 0; shift -= 8) {
                png += static_cast<char>((v >> shift) & 0xff);
            }
        };
        be32(static_cast<u32>(data.size()));
        const std::size_t start = png.size();
        png.append(type, 4);
        png.append(data.begin(), data.end());
        be32(static_cast<u32>(crc32(0, reinterpret_cast<const Bytef*>(png.data() + start),
                                    static_cast<uInt>(png.size() - start))));
    };
    const std::vector<u8> header{0, 0, static_cast<u8>(QuadWidth >> 8), static_cast<u8>(QuadWidth & 0xff),
                                 0, 0, static_cast<u8>(QuadHeight >> 8), static_cast<u8>(QuadHeight & 0xff),
                                 8, 2, 0, 0, 0};
    chunk("IHDR", header);
    chunk("IDAT", packed);
    chunk("IEND", {});
    const auto path =
        std::filesystem::path{DumpDir()} / fmt::format("invite_quad_{}_{}.png", show, what);
    std::ofstream(path, std::ios::binary).write(png.data(), static_cast<std::streamsize>(png.size()));
    LOG_INFO(ImGui, "Invitation banner: headset picture written to {}", path.string());
    return true;
}

} // namespace

void Show(const std::string& key, const std::string& online_id, bool announce) {
    // Before any headset picture is drawn on another thread, which reads the font unlocked.
    CaptureFont();
    {
        std::scoped_lock lock{g_mutex};
        const auto now = Clock::now();
        Advance(now);
        if (!announce && g.key == key && (g.phase == Phase::In || g.phase == Phase::Live)) {
            return;
        }
        g.phase = Phase::In;
        g.since = now;
        g.key = key;
        g.online_id = online_id;
        g.content = {.xr_hints = g.content.xr_hints};
        for (const char ch : online_id) {
            g.content.name += static_cast<char>(std::toupper(static_cast<u8>(ch)));
        }
        for (std::size_t i = 0; i < g.content.name.size() && i < 2; ++i) {
            g.content.initials += g.content.name[i];
        }
        g.out_bar = -1.0f;
        g.paused_at = {};
        ++g.version;
        ++g.shows;
    }
    LOG_INFO(ImGui, "Invitation banner: {} invited you{}", online_id,
             announce ? "" : " (brought back)");
    RequestAvatar(online_id);
    if (announce) {
        PlayChime();
    }
}

void ShowResult(Answer answer) {
    std::scoped_lock lock{g_mutex};
    const auto now = Clock::now();
    Advance(now);
    if (g.phase == Phase::Hidden) {
        return;
    }
    g.phase = Phase::Result;
    g.since = now;
    g.paused_at = {};
    g.content.result = answer;
    ++g.version;
}

void Leave() {
    std::scoped_lock lock{g_mutex};
    const auto now = Clock::now();
    Advance(now);
    if (g.phase == Phase::In || g.phase == Phase::Live || g.phase == Phase::Result) {
        g.out_bar = g.phase == Phase::Result ? -1.0f : ViewAt(now).bar;
        g.phase = Phase::Out;
        g.since = now;
        g.paused_at = {};
    }
}

void SetFocused(bool focused) {
    std::scoped_lock lock{g_mutex};
    const auto now = Clock::now();
    Advance(now);
    if (g.content.focused == focused) {
        return;
    }
    g.content.focused = focused;
    ++g.version;
    if (focused && g.phase == Phase::Live && !Paused()) {
        Pause(now);
    } else if (!focused && Paused()) {
        g.since += now - g.paused_at;
        g.paused_at = {};
        LOG_INFO(ImGui, "Invitation banner: let go unanswered, the countdown resumes");
    }
}

void SetXrHints(bool xr) {
    std::scoped_lock lock{g_mutex};
    if (g.content.xr_hints != xr) {
        g.content.xr_hints = xr;
        ++g.version;
    }
}

std::string OfferedKey() {
    std::scoped_lock lock{g_mutex};
    Advance(Clock::now());
    return g.phase == Phase::In || g.phase == Phase::Live ? g.key : std::string{};
}

bool IsActive() {
    std::scoped_lock lock{g_mutex};
    Advance(Clock::now());
    return g.phase != Phase::Hidden;
}

bool ComposeQuad(u8* pixels, bool bgra, bool fresh, u64& key, float& lift) {
    std::scoped_lock lock{g_mutex};
    return ComposeLocked(pixels, bgra, fresh, key, lift);
}

Answer DrawDesktop() {
    CaptureFont();
    View v;
    Content content;
    std::string online_id;
    Phase phase;
    float in_phase;
    u32 show;
    {
        std::scoped_lock lock{g_mutex};
        const auto now = Clock::now();
        Advance(now);
        v = ViewAt(now);
        content = g.content;
        online_id = g.online_id;
        phase = g.phase;
        in_phase = Seconds(g.since, now);
        show = g.shows;
    }
    if (!v.visible) {
        return Answer::None;
    }

    ImTextureID picture{};
    if (const std::shared_ptr<Avatar> avatar = FindAvatar(online_id);
        avatar && avatar->status == Avatar::Status::Ready) {
        if (!avatar->texture_made) {
            avatar->texture_made = true;
            avatar->texture = RefCountedTexture::DecodePngTexture(avatar->png);
        }
        picture = avatar->texture.GetTexture().im_id;
    }

    const ImGuiIO& io = ImGui::GetIO();
    const float s = io.DisplaySize.x / 1920.0f;
    const ImVec2 size{PanelW * s, PanelH * s};
    const ImVec2 pos{(io.DisplaySize.x - size.x) / 2.0f, 20.0f * s - v.lift * size.y};

    ImGui::SetNextWindowPos(pos, ImGuiCond_Always);
    ImGui::SetNextWindowSize(size, ImGuiCond_Always);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, {0, 0});
    ImGui::PushStyleVar(ImGuiStyleVar_WindowBorderSize, 0.0f);
    constexpr ImGuiWindowFlags flags =
        ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoMove |
        ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoNav |
        ImGuiWindowFlags_NoFocusOnAppearing | ImGuiWindowFlags_NoScrollbar;
    Answer clicked = Answer::None;
    if (ImGui::Begin("##invite_banner", nullptr, flags)) {
        ImGuiCanvas canvas(ImGui::GetWindowDrawList(), pos, s, v.opacity, io.FontDefault != nullptr ? io.FontDefault : ImGui::GetFont(), picture);
        const Hits hits = PaintBody(canvas, content);
        if (v.bar >= 0.0f) {
            PaintBar(canvas, v.bar);
        }
        if (content.result == Answer::None && (phase == Phase::In || phase == Phase::Live)) {
            const auto button = [&](const char* id, Box r) {
                ImGui::SetCursorScreenPos(canvas.Min(r));
                const ImVec2 max = canvas.Max(r);
                const ImVec2 min = canvas.Min(r);
                return ImGui::InvisibleButton(id, {max.x - min.x, max.y - min.y});
            };
            if (button("##invite_accept", hits.accept)) {
                clicked = Answer::Accept;
            }
            if (button("##invite_decline", hits.decline)) {
                clicked = Answer::Decline;
            }
        }
    }
    ImGui::End();
    ImGui::PopStyleVar(2);

    if (DumpDir() != nullptr) {
        static u32 dumped_live = 0;
        static u32 dumped_result = 0;
        static u32 dumped_out = 0;
        if (phase == Phase::Live && in_phase >= 2.0f && dumped_live != show) {
            dumped_live = DumpQuad("live", show) ? show : dumped_live;
        } else if (phase == Phase::Result && in_phase >= 0.5f && dumped_result != show) {
            dumped_result = DumpQuad("result", show) ? show : dumped_result;
        } else if (phase == Phase::Out && in_phase >= 0.08f && dumped_out != show) {
            dumped_out = DumpQuad("leaving", show) ? show : dumped_out;
        }
    }
    return clicked;
}

} // namespace ImGui::InviteBanner
