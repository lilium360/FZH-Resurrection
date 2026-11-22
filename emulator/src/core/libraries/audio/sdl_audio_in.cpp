// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#ifdef _WIN32
#include <windows.h>

#include <audioclient.h>
#include <mmdeviceapi.h>
#include <objbase.h>
#include <propidl.h>
#endif
#include <SDL3/SDL.h>
#include <common/logging/log.h>
#include <common/thread.h>
#include <core/emulator_settings.h>
#include "audioin.h"
#include "audioin_backend.h"
#ifdef ENABLE_OPENXR_HOST
#include "core/vr/openxr_host.h"
#endif

namespace Libraries::AudioIn {

namespace {

using Clock = std::chrono::steady_clock;

constexpr auto RetryAfter = std::chrono::seconds{5};
constexpr auto SaidAfter = std::chrono::seconds{2};

#ifdef _WIN32
/// Opens and starts a recording device (the system's default for an empty name) the way SDL is
/// about to, on the calling thread alone. A device can take forever over that (a headset's
/// virtual microphone while no headset is there), and SDL asks all its devices on one thread of
/// its own: waiting there, it would take the sound the game plays with it.
void TryRecordingDevice(const std::string& name) {
    static constexpr PROPERTYKEY FriendlyName{
        {0xa45c254e, 0xdf1c, 0x4efd, {0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0}}, 14};

    const HRESULT initialized = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    IMMDevice* device = nullptr;
    IMMDeviceEnumerator* enumerator = nullptr;
    if (SUCCEEDED(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                                   __uuidof(IMMDeviceEnumerator),
                                   reinterpret_cast<void**>(&enumerator)))) {
        IMMDeviceCollection* devices = nullptr;
        if (name.empty()) {
            enumerator->GetDefaultAudioEndpoint(eCapture, eConsole, &device);
        } else if (SUCCEEDED(enumerator->EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE,
                                                            &devices))) {
            const int size = MultiByteToWideChar(CP_UTF8, 0, name.c_str(), -1, nullptr, 0);
            std::wstring wanted(static_cast<size_t>(std::max(size, 1)), L'\0');
            MultiByteToWideChar(CP_UTF8, 0, name.c_str(), -1, wanted.data(), size);
            wanted.pop_back();
            UINT count = 0;
            devices->GetCount(&count);
            for (UINT i = 0; i < count && device == nullptr; ++i) {
                IMMDevice* candidate = nullptr;
                if (FAILED(devices->Item(i, &candidate)) || candidate == nullptr) {
                    continue;
                }
                IPropertyStore* properties = nullptr;
                if (SUCCEEDED(candidate->OpenPropertyStore(STGM_READ, &properties))) {
                    PROPVARIANT value;
                    PropVariantInit(&value);
                    if (SUCCEEDED(properties->GetValue(FriendlyName, &value)) &&
                        value.vt == VT_LPWSTR && value.pwszVal != nullptr &&
                        wanted == value.pwszVal) {
                        device = candidate;
                        device->AddRef();
                    }
                    PropVariantClear(&value);
                    properties->Release();
                }
                candidate->Release();
            }
            devices->Release();
        }
        enumerator->Release();
    }
    if (device != nullptr) {
        IAudioClient* client = nullptr;
        if (SUCCEEDED(device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr,
                                       reinterpret_cast<void**>(&client))) &&
            client != nullptr) {
            WAVEFORMATEX* format = nullptr;
            if (SUCCEEDED(client->GetMixFormat(&format)) && format != nullptr) {
                if (SUCCEEDED(client->Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 0, 0, format,
                                                 nullptr)) &&
                    SUCCEEDED(client->Start())) {
                    client->Stop();
                }
                CoTaskMemFree(format);
            }
            client->Release();
        }
        device->Release();
    }
    if (SUCCEEDED(initialized)) {
        CoUninitialize();
    }
}
#endif

/// What a port and the thread that opens its device share. The thread outlives a port whose
/// device never answers.
struct Recording {
    std::atomic<SDL_AudioStream*> stream{nullptr};
    std::atomic<bool> answered{false};
    std::mutex mutex;
    std::condition_variable wake;
    bool closing = false;
};

/// Opens the recording device and keeps it until the port is closed, trying again every few
/// seconds while it does not open. Not on the title's thread: there is no telling how long a
/// device takes, and closing one can take as long as opening it.
void KeepDevice(std::shared_ptr<Recording> recording, SDL_AudioSpec fmt,
                std::optional<SDL_AudioDeviceID> chosen) {
    Common::SetCurrentThreadName("shadPS4:MicOpen");
    // SHADPS4_MIC_OPEN_AFTER=<seconds>: a device that takes that long to open, for tests.
    if (const char* after = std::getenv("SHADPS4_MIC_OPEN_AFTER"); after != nullptr) {
        std::unique_lock lock{recording->mutex};
        if (recording->wake.wait_for(lock, std::chrono::duration<double>(std::atof(after)),
                                     [&] { return recording->closing; })) {
            return;
        }
    }
    bool failure_said = false;
    for (;;) {
        SDL_AudioDeviceID id = chosen.value_or(SDL_AUDIO_DEVICE_DEFAULT_RECORDING);
        std::string name;
#ifdef ENABLE_OPENXR_HOST
        // Left to the system, the microphone is the headset's, if the runtime of this
        // machine's headset names a device for it.
        if (const std::string headset = Core::Vr::OpenXrHost::Instance().AudioInputName();
            !chosen && !headset.empty()) {
            int count = 0;
            SDL_AudioDeviceID* devices = SDL_GetAudioRecordingDevices(&count);
            for (int i = 0; devices != nullptr && i < count; ++i) {
                const char* device_name = SDL_GetAudioDeviceName(devices[i]);
                if (device_name != nullptr && headset == device_name) {
                    id = devices[i];
                    break;
                }
            }
            SDL_free(devices);
        }
#endif
        if (id != SDL_AUDIO_DEVICE_DEFAULT_RECORDING) {
            const char* device_name = SDL_GetAudioDeviceName(id);
            name = device_name != nullptr ? device_name : "";
        }
#ifdef _WIN32
        if (id == SDL_AUDIO_DEVICE_DEFAULT_RECORDING || !name.empty()) {
            TryRecordingDevice(name);
        }
#endif
        SDL_AudioStream* stream = SDL_OpenAudioDeviceStream(id, &fmt, nullptr, nullptr);
        std::string error = stream != nullptr ? "" : SDL_GetError();
        if (stream != nullptr) {
            if (const char* opened = SDL_GetAudioDeviceName(SDL_GetAudioStreamDevice(stream));
                opened != nullptr) {
                name = opened;
            }
            if (!SDL_ResumeAudioStreamDevice(stream)) {
                error = SDL_GetError();
                SDL_DestroyAudioStream(stream);
                stream = nullptr;
            }
        }

        std::unique_lock lock{recording->mutex};
        recording->answered = true;
        if (stream != nullptr) {
            LOG_INFO(Lib_AudioIn, "Audio input opened: {}, {} Hz, {} channels", name, fmt.freq,
                     fmt.channels);
            recording->stream = stream;
            recording->wake.wait(lock, [&] { return recording->closing; });
            recording->stream = nullptr;
            lock.unlock();
            SDL_DestroyAudioStream(stream);
            return;
        }
        if (!failure_said) {
            failure_said = true;
            LOG_ERROR(Lib_AudioIn,
                      "The microphone does not open ({}): the game gets silence, and the "
                      "microphone as soon as it opens",
                      error);
        }
        if (recording->wake.wait_for(lock, RetryAfter, [&] { return recording->closing; })) {
            return;
        }
    }
}

} // namespace

class SDLInPortBackend : public PortInBackend {
public:
    explicit SDLInPortBackend(const PortIn& port) : port(port) {
        SDL_AudioFormat sampleFormat = SDL_AUDIO_S16; // PS4 uses S16 format

        SDL_AudioSpec fmt;
        SDL_zero(fmt);
        fmt.format = sampleFormat;
        fmt.channels = static_cast<Uint8>(port.channels_num);
        fmt.freq = static_cast<int>(port.freq);

        std::string micDevStr = EmulatorSettings.GetSDLMicDevice();
        std::optional<SDL_AudioDeviceID> devId;
        if (micDevStr == "None") {
            nullDevice = true;
            LOG_INFO(Lib_AudioIn, "Audio input disabled by configuration");
        } else if (micDevStr == "Default Device") {
            LOG_INFO(Lib_AudioIn, "Using default audio input device");
        } else {
            try {
                devId = static_cast<SDL_AudioDeviceID>(std::stoul(micDevStr));
                LOG_INFO(Lib_AudioIn, "Using audio input device ID: {}", *devId);
            } catch (const std::exception& e) {
                nullDevice = true;
                LOG_WARNING(Lib_AudioIn, "Invalid device ID '{}', disabling input", micDevStr);
            }
        }

        if (!nullDevice) {
            recording = std::make_shared<Recording>();
            asked = Clock::now();
            std::thread{KeepDevice, recording, fmt, devId}.detach();
        }
    }

    ~SDLInPortBackend() override {
        if (recording) {
            {
                std::scoped_lock lock{recording->mutex};
                recording->closing = true;
            }
            recording->wake.notify_all();
        }
    }

    int Read(void* out_buffer) override {
        const int bytesToRead = port.samples_num * port.sample_size * port.channels_num;

        if (!recording) {
            // Return silence from null device
            std::memset(out_buffer, 0, bytesToRead);
            return port.samples_num;
        }

        SDL_AudioStream* stream = recording->stream.load();
        if (!stream) {
            if (!waiting_said && !recording->answered && Clock::now() - asked > SaidAfter) {
                waiting_said = true;
                LOG_WARNING(Lib_AudioIn, "The microphone has not opened yet: the game goes on "
                                         "with silence, and gets the microphone if it opens");
            }
            return Silence(out_buffer, bytesToRead);
        }

        // Read from actual audio device. One that gives nothing (it happens: a virtual
        // microphone with nobody behind it) is waited for once, a tenth of a second; from
        // then on the title gets silence at the pace of a microphone until sound comes.
        int attempts = 0;
        while (SDL_GetAudioStreamAvailable(stream) < bytesToRead) {
            if (starving || ++attempts > 100) {
                if (!starving) {
                    starving = true;
                    LOG_WARNING(Lib_AudioIn, "The recording device gives no sound: the "
                                             "title gets silence until it does");
                }
                return Silence(out_buffer, bytesToRead);
            }
            SDL_Delay(1);
        }
        starving = false;

        const int bytesRead = SDL_GetAudioStreamData(stream, out_buffer, bytesToRead);
        if (bytesRead < 0) {
            LOG_ERROR(Lib_AudioIn, "Audio input read error: {}", SDL_GetError());
            return 0;
        }

        const int framesRead = bytesRead / (port.sample_size * port.channels_num);
        return framesRead;
    }

    void Clear() override {
        if (SDL_AudioStream* stream = recording ? recording->stream.load() : nullptr) {
            SDL_ClearAudioStream(stream);
        }
    }
    bool IsAvailable() override {
        if (nullDevice) {
            return false;
        } else {
            return true;
        }
    }

private:
    /// Silence for as long as a microphone takes to hear that much. (By the clock: sleeps are
    /// longer than asked for, and are made up for within a twentieth of a second.)
    int Silence(void* out_buffer, int bytes) {
        std::memset(out_buffer, 0, bytes);
        const std::chrono::nanoseconds length{1'000'000'000LL * port.samples_num /
                                              std::max<u32>(port.freq, 1)};
        silent_until =
            std::max(silent_until, Clock::now() - std::chrono::milliseconds{50}) + length;
        std::this_thread::sleep_until(silent_until);
        return port.samples_num;
    }

    const PortIn& port;
    std::shared_ptr<Recording> recording;
    Clock::time_point asked{};
    Clock::time_point silent_until{};
    bool nullDevice = false;
    bool starving = false;
    bool waiting_said = false;
};

std::unique_ptr<PortInBackend> SDLAudioIn::Open(PortIn& port) {
    return std::make_unique<SDLInPortBackend>(port);
}

} // namespace Libraries::AudioIn
