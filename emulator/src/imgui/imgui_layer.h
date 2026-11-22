// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

namespace ImGui {

class Layer {
public:
    virtual ~Layer() = default;
    static void AddLayer(Layer* layer);
    static void RemoveLayer(Layer* layer);

    virtual void Draw() = 0;

    // False while the layer has nothing to show: it is still drawn every frame, but it does not
    // by itself make the emulator redraw (ImGui::Core::MustKeepDrawing).
    virtual bool ShouldKeepDrawing() {
        return true;
    }
};

} // namespace ImGui