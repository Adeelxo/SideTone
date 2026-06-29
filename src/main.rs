#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::cell::Cell;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rodio::Source;
use serde::{Deserialize, Serialize};
use slint::{
    ComponentHandle, LogicalSize, Model, ModelRc, SharedString, Timer, TimerMode, VecModel,
};

use cpal::traits::{DeviceTrait, HostTrait};

mod audio_pipeline;
mod domain;
mod downloads;
mod imports;
mod library;
mod migration;
mod persistence;
mod player;
mod playlists;
mod streaming;
mod streaming_helpers;
mod updates;
use audio_pipeline::*;
use domain::*;
use downloads::*;
use imports::*;
use library::*;
#[cfg(windows)]
use migration::migrate_legacy_data_if_needed;
use persistence::*;
use player::*;
use playlists::*;
use streaming::*;
use streaming_helpers::*;
use updates::*;

type AppResult<T> = Result<T, Box<dyn Error>>;

slint::slint! {
    import { Button, LineEdit, Slider } from "std-widgets.slint";

    export struct QueueRow {
        title: string,
        url: string,
        active: bool,
        is-remote: bool,
        downloaded: bool,
        selected: bool,
    }

    export struct OutputRow {
        title: string,
        index: int,
        active: bool,
    }

    export struct HotkeyRow {
        label: string,
        combo: string,
        capturing: bool,
    }

    // SideTone v4 light earthy palette.
    // Warm neutral utility materials: sandstone shell, linen cards, walnut dock.
    export global Palette {
        in-out property <int> theme: 0;

        // Theme 0: Clay, Theme 1: Ocean Blue, Theme 2: Lilac, Theme 3: Sage.
        // ONE method for ALL four themes: a paper base carrying only a FAINT tint
        // of the theme hue (subtle enough to read as tinted paper, not a colored
        // wash), the dock carrying a touch more of that hue as the "material"
        // layer, and the identity carried by ACCENTS â€” a clean primary hue
        // (clay/logo-a/tab/active) plus a warm COMPLEMENTARY secondary
        // (moss/logo-b â†’ FOCUS). Keep the base chroma low; push character into
        // the accents. (Clay's secondary is moss-green; the cool themes' is warm.)
        out property <color> window: theme == 1 ? #dcdfe5 : (theme == 2 ? #e2dde7 : (theme == 3 ? #dee1d7 : #ece4d4));
        out property <color> surface: theme == 1 ? #eef1f6 : (theme == 2 ? #efeaf5 : (theme == 3 ? #ecefe3 : #f6f0dd));
        out property <color> surface-lift: theme == 1 ? #f3f5f9 : (theme == 2 ? #f4f0f8 : (theme == 3 ? #eef1da : #faf4dd));
        out property <color> surface-hi: theme == 1 ? #f7f9fc : (theme == 2 ? #f8f4fb : (theme == 3 ? #f4f7da : #fcf6df));
        out property <color> hairline: theme == 1 ? #3a4d5f38 : (theme == 2 ? #463a5c38 : (theme == 3 ? #44513238 : #4a371f38));
        out property <color> hairline-hi: theme == 1 ? #3a6f9a87 : (theme == 2 ? #7d52a487 : (theme == 3 ? #5d7a3f87 : #9e623987));
        out property <color> shadow: theme == 1 ? #3a4d5f20 : (theme == 2 ? #463a5c20 : (theme == 3 ? #44513220 : #4a371f20));

        out property <color> text: theme == 1 ? #25303a : (theme == 2 ? #2b2533 : (theme == 3 ? #28301f : #2e271f));
        out property <color> text-dim: theme == 1 ? #44515c : (theme == 2 ? #4d4757 : (theme == 3 ? #4a5043 : #51483d));
        out property <color> text-faint: theme == 1 ? #6c757d : (theme == 2 ? #736c7c : (theme == 3 ? #6c7263 : #756a5b));

        out property <color> clay: theme == 1 ? #356d9c : (theme == 2 ? #7d4fa3 : (theme == 3 ? #588040 : #a85f2d));
        out property <color> clay-bright: theme == 1 ? #275986 : (theme == 2 ? #663d8a : (theme == 3 ? #46692f : #8b4b22));
        // Secondary = complementary WARM tone (FOCUS / Output), so each theme
        // reads as two colors playing off each other, not one hue washed out.
        out property <color> moss: theme == 1 ? #bb7d35 : (theme == 2 ? #bd8a3a : (theme == 3 ? #bc6b3a : #627040));
        // Wordmark two-tone: primary follows the theme, secondary echoes the
        // warm complement so the wordmark itself shows both theme colors.
        out property <color> logo-a: theme == 1 ? #2b6088 : (theme == 2 ? #6a3f92 : (theme == 3 ? #4a6f2c : #8b4b22));
        out property <color> logo-b: theme == 1 ? #a8702f : (theme == 2 ? #a8742f : (theme == 3 ? #a85d2f : #627040));
        out property <color> amber: theme == 0 ? #b17f31 : #c0922f;
        out property <color> amber-bright: theme == 0 ? #795515 : #9c6f1d;

        out property <color> active-bg: theme == 1 ? #ccdbe8 : (theme == 2 ? #dccfe8 : (theme == 3 ? #dde3c8 : #e8dabf));
        out property <color> active-text: theme == 1 ? #1f2c36 : (theme == 2 ? #2a2533 : (theme == 3 ? #28301f : #2c241a));

        out property <color> dock: theme == 1 ? #c3cbd5 : (theme == 2 ? #ccc3d4 : (theme == 3 ? #c8ccb6 : #d4bb9c));
        out property <color> dock-hi: theme == 1 ? #b9c2cd : (theme == 2 ? #c1b8cc : (theme == 3 ? #bdc2aa : #c8ad8c));
        out property <color> dock-text: theme == 1 ? #1f2c36 : (theme == 2 ? #2a2533 : (theme == 3 ? #28301f : #2f271d));
        out property <color> dock-dim: theme == 1 ? #5d6770 : (theme == 2 ? #6a6275 : (theme == 3 ? #696f5a : #6b5238));
        out property <color> dock-line: theme == 1 ? #1f3a4d30 : (theme == 2 ? #2c1b4d30 : (theme == 3 ? #2a3a1830 : #2c190c30));
        out property <color> dock-shadow: theme == 1 ? #1f3a4d38 : (theme == 2 ? #2c1b4d38 : (theme == 3 ? #2a3a1838 : #2c190c38));
        out property <color> control-face: theme == 1 ? #ebf0f6 : (theme == 2 ? #efe9f5 : (theme == 3 ? #ecefe1 : #faecd8));
        out property <color> control-hi: theme == 1 ? #f5f8fb : (theme == 2 ? #f9f4fc : (theme == 3 ? #f4f7da : #fbf2dd));
        out property <color> control-shadow: theme == 1 ? #1f3a4d2a : (theme == 2 ? #2c1b4d2a : (theme == 3 ? #2a3a182a : #2c190c2a));

        out property <color> sheen: theme == 0 ? #fffaf0 : #fffaf2;
        out property <color> dim: theme == 1 ? #122734 : (theme == 2 ? #241733 : (theme == 3 ? #1f2a12 : #3a2810));
        out property <color> glow: theme == 1 ? #8aa6bd : (theme == 2 ? #a98dbd : (theme == 3 ? #97ad7a : #b59a67));
    }

    global Icons {
        out property <string> play: "M 8 5.5 L 18.5 12 L 8 18.5 Z";
        out property <string> pause: "M 9 5.5 L 9 18.5 M 15 5.5 L 15 18.5";
        out property <string> stop: "M 6.5 6.5 L 17.5 6.5 L 17.5 17.5 L 6.5 17.5 Z";
        out property <string> prev: "M 7 5.5 L 7 18.5 M 18 5.5 L 8.5 12 L 18 18.5 Z";
        out property <string> next: "M 17 5.5 L 17 18.5 M 6 5.5 L 15.5 12 L 6 18.5 Z";
        out property <string> speaker: "M 3 9 L 7 9 L 12 5 L 12 19 L 7 15 L 3 15 Z";
        out property <string> folder: "M 3 6.5 L 9 6.5 L 11 9 L 21 9 L 21 18 L 3 18 Z";
        out property <string> download: "M 12 4 L 12 14 M 8 10.5 L 12 14.5 L 16 10.5 M 6 18 L 18 18";
        out property <string> close: "M 7 7 L 17 17 M 17 7 L 7 17";
        out property <string> caret-down: "M 6 9.5 L 12 15 L 18 9.5";
        out property <string> caret-up: "M 6 14.5 L 12 9 L 18 14.5";
        // left-pointing back arrow (stroked)
        out property <string> back: "M 12.5 6 L 6.5 12 L 12.5 18 M 7 12 L 18.5 12";
        // trash can (stroked)
        out property <string> trash: "M 5 7 L 19 7 M 9.5 7 L 9.5 4.5 L 14.5 4.5 L 14.5 7 M 7 7 L 8 19.5 L 16 19.5 L 17 7 M 10 10 L 10 16.5 M 14 10 L 14 16.5";
        out property <string> repeat: "M 17 5 L 20 8 L 17 11 M 20 8 L 7 8 L 5 10 L 5 12 M 7 19 L 4 16 L 7 13 M 4 16 L 17 16 L 19 14 L 19 12";
        out property <string> tune: "M 3.5 12 C 5.1 12 5.5 9 7 9 C 8.8 9 8.8 15 10.5 15 C 12.4 15 12.4 5 14.2 5 C 16.1 5 16.1 19 18 19 C 19.6 19 19.9 12 21.5 12 M 12 7 L 12 17";
        // shuffle: two crossing arrows (stroked)
        out property <string> shuffle: "M 4 7 L 8 7 L 12 12 L 16 17 L 20 17 M 17 4.5 L 20 7.5 L 17 10.5 M 4 17 L 8 17 L 10 14.5 M 14.5 9.5 L 16 7.5 L 20 7.5 M 17 13.5 L 20 16.5 L 17 19.5";
        // settings â€” 8-tooth gear/cog with center hole (stroked)
        out property <string> gear: "M 18.2 10.3 L 21.2 10.6 L 21.2 13.5 L 18.2 13.7 L 17.5 15.2 L 19.5 17.5 L 17.5 19.5 L 15.2 17.5 L 13.7 18.2 L 13.5 21.2 L 10.6 21.2 L 10.3 18.2 L 8.8 17.5 L 6.5 19.5 L 4.5 17.5 L 6.5 15.2 L 5.8 13.7 L 2.8 13.5 L 2.8 10.6 L 5.8 10.3 L 6.5 8.8 L 4.5 6.5 L 6.5 4.5 L 8.8 6.5 L 10.3 5.8 L 10.6 2.8 L 13.5 2.8 L 13.7 5.8 L 15.2 6.5 L 17.5 4.5 L 19.5 6.5 L 17.5 8.8 Z M 14.8 12 A 2.8 2.8 0 1 0 9.2 12 A 2.8 2.8 0 1 0 14.8 12";
        // layout/dashboard: outer frame split into a sidebar + two stacked
        // panels â€” the universal "layouts" glyph (stroked)
        out property <string> layout: "M 4 5.5 L 20 5.5 L 20 18.5 L 4 18.5 Z M 10 5.5 L 10 18.5 M 10 12 L 20 12";
        // magnifier (stroked) â€” search / filter affordance
        out property <string> search: "M 10.5 4 A 6.5 6.5 0 1 0 10.5 17 A 6.5 6.5 0 1 0 10.5 4 M 15.2 15.2 L 20 20";
    }

    // Vector icon â€” authored in a 24x24 viewbox. `outline` strokes instead of fills.
    component Glyph inherits Rectangle {
        in property <string> d;
        in property <color> tint: Palette.text;
        in property <bool> outline: false;
        in property <length> sw: 2px;
        background: transparent;
        Path {
            width: 100%;
            height: 100%;
            commands: root.d;
            viewbox-width: 24;
            viewbox-height: 24;
            fill: root.outline ? transparent : root.tint;
            stroke: root.outline ? root.tint : transparent;
            stroke-width: root.sw;
        }
    }

    component WarmSlider inherits Rectangle {
        in property <float> minimum: 0;
        in property <float> maximum: 100;
        in-out property <float> value: 0;
        in property <bool> enabled: true;
        callback changed(float);

        property <float> span: root.maximum - root.minimum;
        property <float> pct: root.span <= 0 ? 0 : (root.value - root.minimum) / root.span;

        height: 20px;
        background: transparent;

        Rectangle {
            x: 1px; y: (parent.height - self.height) / 2 + 1px;
            width: parent.width - 2px; height: 9px;
            border-radius: 5px;
            background: Palette.dock-shadow.with-alpha(root.enabled ? 0.18 : 0.10);
        }
        Rectangle {
            x: 0px; y: (parent.height - self.height) / 2;
            width: parent.width; height: 9px;
            border-radius: 5px;
            border-width: 1px;
            border-color: root.enabled ? Palette.sheen.with-alpha(0.44) : Palette.dock-line.with-alpha(0.34);
            background: transparent;
        }
        Rectangle {
            x: 1px; y: (parent.height - self.height) / 2 + 1px;
            width: 7px + (parent.width - 9px) * root.pct; height: 7px;
            border-radius: 4px;
            background: @linear-gradient(90deg, Palette.glow.with-alpha(root.enabled ? 0.72 : 0.24) 0%, Palette.clay.with-alpha(root.enabled ? 0.76 : 0.28) 100%);
            Rectangle {
                x: 1px; y: 1px; width: parent.width - 2px; height: 3px;
                border-radius: 2px;
                background: Palette.sheen.with-alpha(root.enabled ? 0.30 : 0.10);
            }
        }
        Rectangle {
            x: 3px + (parent.width - 16px) * root.pct;
            y: (parent.height - self.height) / 2 + 2px;
            width: 16px; height: 16px;
            border-radius: 8px;
            background: Palette.control-shadow;
        }
        Rectangle {
            x: 3px + (parent.width - 16px) * root.pct;
            y: (parent.height - self.height) / 2;
            width: 16px; height: 16px;
            border-radius: 8px;
            background: root.enabled ? @linear-gradient(180deg, Palette.control-hi 0%, Palette.control-face 100%) : Palette.control-face.with-alpha(0.48);
            border-width: 1px;
            border-color: root.enabled ? Palette.sheen.with-alpha(0.72) : Palette.dock-line;
            Rectangle {
                x: 4px; y: 4px; width: parent.width - 8px; height: parent.height - 8px;
                border-radius: 4px;
                background: Palette.sheen.with-alpha(root.enabled ? 0.30 : 0.10);
            }
        }
        Slider {
            width: 100%; height: 100%;
            opacity: 0.01;
            minimum: root.minimum; maximum: root.maximum;
            value: root.value;
            enabled: root.enabled;
            changed(v) => { root.value = v; root.changed(v); }
        }
    }

    component HotkeyCell inherits Rectangle {
        in property <string> label;
        in property <string> combo;
        in property <int> index;
        in property <bool> capturing;
        callback activate(int);

        background: transparent;
        HorizontalLayout {
            width: 100%; height: 100%; spacing: 8px;
            Text {
                horizontal-stretch: 1;
                text: root.label;
                color: Palette.text-dim;
                font-size: 11px; vertical-alignment: center; overflow: elide;
            }
            Rectangle {
                width: 118px; height: 28px;
                y: (parent.height - self.height) / 2;
                border-radius: 8px;
                background: root.capturing ? Palette.amber.with-alpha(0.22)
                    : Palette.active-bg.with-alpha(0.6);
                border-width: 1px;
                border-color: root.capturing ? Palette.clay-bright : Palette.hairline;
                Text {
                    x: 6px; width: parent.width - 12px; height: parent.height;
                    text: root.capturing ? "Press keys..." : root.combo;
                    color: root.capturing ? Palette.clay-bright : Palette.text;
                    font-size: 10px; font-weight: 500;
                    horizontal-alignment: center; vertical-alignment: center; overflow: elide;
                }
                TouchArea { clicked => { root.activate(root.index); } }
            }
        }
    }

    // Retro rotary knob â€” vertical-drag to adjust. Used for Reverb in the TUNE
    // panel. Numbered dial + neumorphic body for that vintage-hardware feel.
    component Knob inherits Rectangle {
        in property <float> minimum: 0;
        in property <float> maximum: 100;
        in-out property <float> value: 0;
        callback changed(float);
        property <float> pct: (root.maximum - root.minimum) <= 0 ? 0
            : (root.value - root.minimum) / (root.maximum - root.minimum);
        property <angle> ang: (root.pct - 0.5) * 270deg;
        property <length> cx: root.width / 2;
        property <length> cy: root.height / 2;
        property <length> rad: (min(root.width, root.height) - 32px) / 2;

        // numbered dial labels
        for t in [0, 20, 40, 60, 80, 100]: Text {
            property <angle> ta: (t / 100.0 - 0.5) * 270deg;
            x: root.cx + (root.rad + 11px) * Math.sin(ta) - self.width / 2;
            y: root.cy - (root.rad + 11px) * Math.cos(ta) - self.height / 2;
            text: t + "";
            color: Palette.text-faint;
            font-size: 8px;
        }
        // active amount arc, built from overlapping glossy beads so it reads as
        // a continuous progress ring at small sizes.
        for s in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
                  13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
                  25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36,
                  37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48]: Rectangle {
            property <float> step-pct: s / 48.0;
            property <angle> sa: (self.step-pct - 0.5) * 270deg;
            visible: root.value > 0.5 && self.step-pct <= root.pct;
            width: 8px; height: 8px;
            x: root.cx + root.rad * Math.sin(sa) - self.width / 2;
            y: root.cy - root.rad * Math.cos(sa) - self.height / 2;
            border-radius: 4px;
            background: @radial-gradient(circle, Palette.sheen.with-alpha(0.38) 0%, Palette.clay 58%, Palette.clay-bright 100%);
        }
        // knob body
        Rectangle {
            width: root.rad * 2; height: root.rad * 2;
            x: root.cx - root.rad; y: root.cy - root.rad;
            border-radius: root.rad;
            background: @radial-gradient(circle, Palette.surface-hi 0%, Palette.surface 70%, Palette.active-bg 100%);
            border-width: 1px; border-color: Palette.hairline;
            Rectangle {
                x: parent.width * 0.18; y: parent.height * 0.1;
                width: parent.width * 0.64; height: parent.height * 0.42;
                border-radius: self.height / 2;
                background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.75) 0%, transparent 100%);
            }
        }
        // indicator dot
        Rectangle {
            property <length> ir: root.rad - 9px;
            width: 7px; height: 7px; border-radius: 4px;
            x: root.cx + ir * Math.sin(root.ang) - 3.5px;
            y: root.cy - ir * Math.cos(root.ang) - 3.5px;
            background: Palette.clay;
        }
        TouchArea {
            property <float> start-val;
            pointer-event(ev) => {
                if (ev.kind == PointerEventKind.down) { self.start-val = root.value; }
            }
            moved => {
                root.value = Math.min(root.maximum, Math.max(root.minimum,
                    self.start-val + (self.pressed-y - self.mouse-y) / 1px * 0.6));
                root.changed(root.value);
            }
        }
    }

    // Phase 2b: the persistent transport dock, extracted so every layout mode
    // (Standard / Mini) can reuse it. Behaviour is identical to the
    // former inline block â€” `root` simply rebinds to this component, so the
    // internal references are unchanged. Bound to AppWindow state at the use site.
    component Transport inherits Rectangle {
        height: 196px;
        background: transparent;

        in property <string> now-title;
        in property <string> progress-text;
        in-out property <float> progress-percent;
        in property <bool> progress-seekable;
        in property <bool> playback-active;
        in property <bool> playback-paused;
        in property <int> repeat-mode;
        in property <bool> shuffle-active;
        in-out property <float> volume;
        in property <bool> tune-active;
        in-out property <bool> tune-open;
        in-out property <bool> focus-enabled;
        in-out property <float> focus-intensity;

        callback seek-changed(float);
        callback queue-prev();
        callback queue-next();
        callback pause-toggle();
        callback shuffle-queue();
        callback repeat-cycle();
        callback volume-changed(float);
        callback panel-layout-changed();
        callback focus-toggle();
        callback focus-intensity-changed(float);

        // Re-push the slider handle after the timer/reset updates progress-percent
        // (dragging severs the slider's `value:` binding). Moved here with the dock.
        changed progress-percent => { progress-slider.value = self.progress-percent; }

        Rectangle {
            x: 10px; y: 8px; width: parent.width - 20px; height: parent.height - 8px;
            border-radius: 18px;
            background: Palette.dock-shadow;
        }
        // The dock stays warm wood in Focus — the global vignette does the
        // dimming, so the tray never turns muddy.
        Rectangle {
            x: 6px; y: 0px; width: parent.width - 12px; height: parent.height - 5px;
            background: @linear-gradient(180deg, Palette.dock-hi 0%, Palette.dock 74%);
            border-radius: 17px;
            border-width: 1px;
            border-color: Palette.dock-line;
        }
        Rectangle {
            x: 8px; y: 2px; width: parent.width - 16px; height: 34px;
            border-radius: 16px;
            background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.32) 0%, transparent 100%);
        }
        VerticalLayout {
            x: 6px; y: 0px; width: parent.width - 12px; height: parent.height - 5px;
            padding-left: 16px; padding-right: 16px; padding-top: 10px; padding-bottom: 10px;
            spacing: 8px;
            // Row 1: now-playing track name (left) + time (right).
            // Small like the timer, but bolder so it reads as the label.
            HorizontalLayout {
                height: 14px; spacing: 8px;
                Text {
                    horizontal-stretch: 1;
                    text: root.now-title;
                    color: Palette.dock-text;
                    font-size: 11px; font-weight: 700; letter-spacing: 0px;
                    vertical-alignment: center; overflow: elide;
                }
                Text {
                    text: root.progress-text;
                    color: Palette.dock-dim;
                    font-size: 10px; font-weight: 500; vertical-alignment: center;
                }
            }
            // Row 2: progress slider. Dragging to seek sets the slider's
            // own `value`, which severs its `value: root.progress-percent`
            // binding — so the timer + reset re-push via `changed
            // progress-percent` below to keep the handle live.
            progress-slider := WarmSlider {
                height: 18px;
                minimum: 0; maximum: 100;
                value: root.progress-percent;
                enabled: root.progress-seekable;
                changed(v) => { root.progress-percent = v; root.seek-changed(v); }
            }
            // Row 3: tactile playback controls (raised wooden buttons),
            // clay play/pause hero, repeat + focus on the right.
            HorizontalLayout {
                height: 44px; spacing: 10px;
                Rectangle { horizontal-stretch: 1; background: transparent; }
                for btn-data in [
                    { d: Icons.tune, act: 4, primary: false },
                    { d: Icons.prev, act: 0, primary: false },
                    { d: (!root.playback-active || root.playback-paused) ? Icons.play : Icons.pause, act: 1, primary: true },
                    { d: Icons.next, act: 2, primary: false },
                    { d: Icons.shuffle, act: 3, primary: false },
                ]: Rectangle {
                    width: 42px; height: 42px;
                    y: (parent.height - self.height) / 2;
                    // contact shadow — shrinks toward the face when pressed
                    // so the button reads as sinking into the dock
                    Rectangle {
                        x: 0; y: tb-ta.pressed ? 0.5px : 2.5px;
                        width: 42px; height: 42px;
                        border-radius: 21px; background: Palette.control-shadow;
                        animate y { duration: 70ms; easing: ease-out; }
                    }
                    // raised face (dips down on press)
                    Rectangle {
                        width: 42px; height: 42px; border-radius: 21px;
                        y: tb-ta.pressed ? 1.5px : 0px;
                        animate y { duration: 70ms; easing: ease-out; }
                        background: (btn-data.primary || (btn-data.act == 3 && root.shuffle-active) || (btn-data.act == 4 && root.tune-active))
                            ? (tb-ta.pressed ? Palette.clay-bright : Palette.clay)
                            : (tb-ta.pressed ? Palette.dock-hi : (tb-ta.has-hover ? Palette.control-hi : Palette.control-face));
                        border-width: 1px;
                        border-color: (btn-data.primary || (btn-data.act == 3 && root.shuffle-active) || (btn-data.act == 4 && root.tune-active)) ? Palette.clay-bright : Palette.dock-line;
                        // glossy top — a FULL circle with a top-weighted fade,
                        // so the highlight hugs the edge instead of chipping
                        Rectangle {
                            x: 1px; y: 1px; width: 40px; height: 40px;
                            border-radius: 20px;
                            background: @linear-gradient(180deg, Palette.sheen.with-alpha((btn-data.primary || (btn-data.act == 3 && root.shuffle-active) || (btn-data.act == 4 && root.tune-active)) ? 0.30 : 0.55) 0%, transparent 54%);
                        }
                        Glyph {
                            width: btn-data.primary ? 22px : (btn-data.act == 4 ? 22px : 19px);
                            height: btn-data.primary ? 22px : (btn-data.act == 4 ? 22px : 19px);
                            x: (parent.width - self.width) / 2;
                            y: (parent.height - self.height) / 2;
                            d: btn-data.d;
                            tint: (btn-data.primary || (btn-data.act == 3 && root.shuffle-active) || (btn-data.act == 4 && root.tune-active)) ? Palette.sheen : Palette.dock-text;
                            outline: true; sw: btn-data.act == 4 ? 1.6px : 1.9px;
                        }
                    }
                    tb-ta := TouchArea {
                        width: 42px; height: 42px;
                        clicked => {
                            if btn-data.act == 0 { root.queue-prev(); }
                            if btn-data.act == 1 { root.pause-toggle(); }
                            if btn-data.act == 2 { root.queue-next(); }
                            if btn-data.act == 3 { root.shuffle-queue(); }
                            if btn-data.act == 4 { root.tune-open = !root.tune-open; root.panel-layout-changed(); }
                        }
                    }
                }
                // Repeat — tactile button. Active state matches the clay
                // play hero (clay face + cream glyph) so the accent reads
                // consistently. "1" badge = repeat-one.
                Rectangle {
                    width: 38px; height: 38px;
                    y: (parent.height - self.height) / 2;
                    Rectangle {
                        x: 0; y: rpt-ta.pressed ? 0.5px : 2.5px;
                        width: 38px; height: 38px;
                        border-radius: 19px; background: Palette.control-shadow;
                        animate y { duration: 70ms; easing: ease-out; }
                    }
                    Rectangle {
                        width: 38px; height: 38px; border-radius: 19px;
                        y: rpt-ta.pressed ? 1.5px : 0px;
                        animate y { duration: 70ms; easing: ease-out; }
                        background: root.repeat-mode != 0
                            ? (rpt-ta.pressed ? Palette.clay-bright : Palette.clay)
                            : (rpt-ta.pressed ? Palette.dock-hi : (rpt-ta.has-hover ? Palette.control-hi : Palette.control-face));
                        border-width: 1px;
                        border-color: root.repeat-mode != 0 ? Palette.clay-bright : Palette.dock-line;
                        Rectangle {
                            x: 1px; y: 1px; width: 36px; height: 36px;
                            border-radius: 18px;
                            background: @linear-gradient(180deg, Palette.sheen.with-alpha(root.repeat-mode != 0 ? 0.28 : 0.45) 0%, transparent 54%);
                        }
                        Glyph {
                            width: 19px; height: 19px;
                            x: (parent.width - self.width) / 2;
                            y: (parent.height - self.height) / 2;
                            d: Icons.repeat;
                            tint: root.repeat-mode != 0 ? Palette.sheen : Palette.dock-dim;
                            outline: true; sw: 1.8px;
                        }
                        // repeat-one badge: a crisp filled disc with a sheen ring
                        // so it reads as a clean "1" pip, not a chipped corner.
                        if root.repeat-mode == 2: Rectangle {
                            x: parent.width - 16px; y: parent.height - 16px;
                            width: 15px; height: 15px; border-radius: 8px;
                            background: Palette.clay-bright;
                            border-width: 1.5px; border-color: Palette.sheen;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "1"; color: Palette.sheen;
                                font-family: "Inter"; font-size: 9px; font-weight: 700;
                                horizontal-alignment: center; vertical-alignment: center;
                            }
                        }
                    }
                    rpt-ta := TouchArea { width: 38px; height: 38px; clicked => { root.repeat-cycle(); } }
                }
                Rectangle { horizontal-stretch: 1; background: transparent; }
            }
            HorizontalLayout {
                height: 22px; spacing: 8px;
                Rectangle { horizontal-stretch: 1; background: transparent; }
                Rectangle {
                    width: 18px; height: parent.height; background: transparent;
                    Glyph {
                        width: 16px; height: 16px;
                        x: (parent.width - self.width)/2; y: (parent.height - self.height)/2;
                        d: Icons.speaker; tint: Palette.dock-dim;
                    }
                }
                WarmSlider {
                    width: 210px; height: 20px;
                    minimum: 0; maximum: 100;
                    value: root.volume;
                    changed(v) => { root.volume = v; root.volume-changed(v); }
                }
                Rectangle { horizontal-stretch: 1; background: transparent; }
            }
            // Row 5: FOCUS dominates the dock as the signature feature. TUNE
            // lives in the icon row above as a smaller waveform control.
            HorizontalLayout {
                height: 44px; spacing: 16px;
                Rectangle { horizontal-stretch: 1; background: transparent; }
                // FOCUS — clay when on. The hero control: wider, with its own
                // exclusive Bahnschrift type so it reads as the signature feature.
                Rectangle {
                    width: root.focus-enabled ? 276px : 156px;
                    min-height: root.focus-enabled ? 44px : 40px;
                    max-height: root.focus-enabled ? 44px : 40px;
                    animate width, min-height, max-height { duration: 220ms; easing: ease-out; }
                    Rectangle {
                        x: 0; y: focus-ta.pressed ? 0.5px : 2.5px;
                        width: parent.width; height: parent.height; border-radius: parent.height / 2;
                        background: Palette.control-shadow;
                        animate y { duration: 70ms; easing: ease-out; }
                    }
                    Rectangle {
                        width: parent.width; height: parent.height; border-radius: parent.height / 2;
                        y: focus-ta.pressed ? 1.5px : 0px;
                        animate y { duration: 70ms; easing: ease-out; }
                        background: root.focus-enabled
                            ? (focus-ta.pressed ? Palette.clay-bright : Palette.clay)
                            : (focus-ta.pressed ? Palette.dock-hi : (focus-ta.has-hover ? Palette.control-hi : Palette.control-face));
                        border-width: 1px;
                        border-color: root.focus-enabled ? Palette.clay-bright : Palette.dock-line;
                        Rectangle {
                            x: 1px; y: 1px; width: parent.width - 2px; height: parent.height - 2px;
                            border-radius: self.height / 2;
                            background: @linear-gradient(180deg, Palette.sheen.with-alpha(root.focus-enabled ? 0.30 : 0.58) 0%, transparent 54%);
                        }
                        if root.focus-enabled: Rectangle {
                            x: 54px; y: 32px; width: parent.width - 108px; height: 3px;
                            border-radius: 2px;
                            background: Palette.sheen.with-alpha(0.16);
                            Rectangle {
                                width: root.focus-intensity < 50 ? (parent.width / 2 - parent.width * root.focus-intensity / 100) : 0px;
                                height: parent.height;
                                x: parent.width * root.focus-intensity / 100;
                                background: Palette.sheen.with-alpha(0.18);
                                border-radius: 2px;
                            }
                            Rectangle {
                                width: root.focus-intensity > 50 ? (parent.width * (root.focus-intensity - 50) / 100) : 0px;
                                height: parent.height;
                                x: parent.width / 2;
                                background: Palette.sheen.with-alpha(0.18);
                                border-radius: 2px;
                            }
                            Rectangle {
                                x: parent.width / 2 - 1px; y: -4px; width: 2px; height: 12px;
                                border-radius: 1px;
                                background: Palette.sheen.with-alpha(0.42);
                            }
                            Rectangle {
                                x: Math.min(parent.width - 14px, Math.max(0px, parent.width * root.focus-intensity / 100 - 7px));
                                y: -4px; width: 11px; height: 11px; border-radius: 6px;
                                background: Palette.sheen.with-alpha(0.62);
                                border-width: 1px; border-color: Palette.sheen.with-alpha(0.26);
                            }
                        }
                        if root.focus-enabled: Text {
                            x: 8px; y: 21px; width: 46px; height: 12px;
                            text: "clear";
                            color: Palette.sheen.with-alpha(0.92);
                            font-size: 9px; font-weight: 700; letter-spacing: 1.1px;
                            horizontal-alignment: center; vertical-alignment: center;
                        }
                        if root.focus-enabled: Text {
                            x: parent.width - 54px; y: 21px; width: 46px; height: 12px;
                            text: "muffle";
                            color: Palette.sheen.with-alpha(0.92);
                            font-size: 9px; font-weight: 700; letter-spacing: 1.1px;
                            horizontal-alignment: center; vertical-alignment: center;
                        }
                        Text {
                            width: parent.width; height: parent.height;
                            x: 0px; y: root.focus-enabled ? -6px : 1px;
                            text: "FOCUS";
                            color: root.focus-enabled ? transparent : Palette.sheen.with-alpha(0.42);
                            font-family: "Bahnschrift SemiBold";
                            font-size: root.focus-enabled ? 13px : 15px; font-weight: 600; letter-spacing: root.focus-enabled ? 4.2px : 2px;
                            horizontal-alignment: center; vertical-alignment: center;
                            animate y, font-size, letter-spacing { duration: 180ms; easing: ease-out; }
                        }
                        if root.focus-enabled: Text {
                            width: parent.width; height: parent.height;
                            x: -4px; y: -8px;
                            text: "FOCUS";
                            color: Palette.sheen.with-alpha(0.14);
                            font-family: "Bahnschrift SemiBold";
                            font-size: 13px; font-weight: 600; letter-spacing: 4.2px;
                            horizontal-alignment: center; vertical-alignment: center;
                        }
                        if root.focus-enabled: Text {
                            width: parent.width; height: parent.height;
                            x: 4px; y: -5px;
                            text: "FOCUS";
                            color: Palette.clay-bright.with-alpha(0.16);
                            font-family: "Bahnschrift SemiBold";
                            font-size: 13px; font-weight: 600; letter-spacing: 4.2px;
                            horizontal-alignment: center; vertical-alignment: center;
                        }
                        Text {
                            width: parent.width; height: parent.height;
                            y: root.focus-enabled ? -6px : 0px;
                            text: "FOCUS";
                            color: root.focus-enabled ? Palette.sheen : Palette.logo-a;
                            font-family: "Bahnschrift SemiBold";
                            font-size: root.focus-enabled ? 13px : 15px; font-weight: 600; letter-spacing: root.focus-enabled ? 4.2px : 2px;
                            horizontal-alignment: center; vertical-alignment: center;
                            animate y, font-size, letter-spacing { duration: 180ms; easing: ease-out; }
                        }
                    }
                    focus-ta := TouchArea {
                        property <float> drag-start-x;
                        width: parent.width; height: parent.height;
                        pointer-event(ev) => {
                            if (ev.kind == PointerEventKind.down) {
                                self.drag-start-x = self.mouse-x / 1px;
                            }
                        }
                        moved => {
                            if (root.focus-enabled) {
                                root.focus-intensity = Math.min(100, Math.max(0, (self.mouse-x - 54px) / (parent.width - 108px) * 100));
                                root.focus-intensity-changed(root.focus-intensity);
                            }
                        }
                        clicked => {
                            if (!root.focus-enabled || Math.abs(self.mouse-x / 1px - self.drag-start-x) < 3) {
                                root.focus-enabled = !root.focus-enabled;
                                root.focus-toggle();
                            }
                        }
                    }
                }
                Rectangle { horizontal-stretch: 1; background: transparent; }
            }
        }
    }

    // Phase 2b: Mini layout â€” a thin always-visible control strip for when the
    // app is parked beside a game. Title + transport + Focus + volume in one row.
    // Normal window (not always-on-top, by design). Bound to AppWindow below.
    component MiniBar inherits Rectangle {
        height: 96px;
        background: transparent;

        in property <string> now-title;
        in property <string> progress-text;
        in property <bool> playback-active;
        in property <bool> playback-paused;
        in-out property <float> volume;
        in-out property <bool> focus-enabled;

        callback queue-prev();
        callback queue-next();
        callback pause-toggle();
        callback focus-toggle();
        callback volume-changed(float);
        callback cycle-layout();

        // Walnut dock backdrop, same family as the full transport.
        Rectangle {
            x: 8px; y: 8px; width: parent.width - 16px; height: parent.height - 16px;
            border-radius: 16px;
            background: @linear-gradient(180deg, Palette.dock-hi 0%, Palette.dock 74%);
            border-width: 1px; border-color: Palette.dock-line;
        }
        HorizontalLayout {
            x: 18px; width: parent.width - 36px; height: parent.height;
            spacing: 14px;
            // â—  â–·/âšâš  â–·|  â€” transport on the left, vertically centered.
            for btn in [
                { d: Icons.prev, act: 0, primary: false },
                { d: (!root.playback-active || root.playback-paused) ? Icons.play : Icons.pause, act: 1, primary: true },
                { d: Icons.next, act: 2, primary: false },
            ]: Rectangle {
                width: btn.primary ? 44px : 36px;
                height: btn.primary ? 44px : 36px;
                y: (parent.height - self.height) / 2;
                Rectangle {
                    width: parent.width; height: parent.height;
                    border-radius: parent.width / 2;
                    background: btn.primary
                        ? (mb-ta.pressed ? Palette.clay-bright : Palette.clay)
                        : (mb-ta.pressed ? Palette.dock-hi : (mb-ta.has-hover ? Palette.control-hi : Palette.control-face));
                    border-width: 1px;
                    border-color: btn.primary ? Palette.clay-bright : Palette.dock-line;
                    Glyph {
                        width: btn.primary ? 22px : 18px;
                        height: btn.primary ? 22px : 18px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        d: btn.d;
                        tint: btn.primary ? Palette.sheen : Palette.dock-text;
                        outline: true; sw: 1.9px;
                    }
                }
                mb-ta := TouchArea {
                    clicked => {
                        if btn.act == 0 { root.queue-prev(); }
                        if btn.act == 1 { root.pause-toggle(); }
                        if btn.act == 2 { root.queue-next(); }
                    }
                }
            }
            // Title + time â€” centered in the middle.
            VerticalLayout {
                horizontal-stretch: 1;
                alignment: center; spacing: 2px;
                Text {
                    text: root.now-title;
                    color: Palette.dock-text;
                    font-size: 12px; font-weight: 700;
                    horizontal-alignment: center;
                    vertical-alignment: center; overflow: elide;
                }
                Text {
                    text: root.progress-text;
                    color: Palette.dock-dim;
                    font-size: 10px; font-weight: 500;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }
            // Right cluster: FOCUS stacked above volume, vertically centered.
            VerticalLayout {
                width: 150px;
                alignment: center; spacing: 8px;
                // FOCUS â€” clay when on. Narrow pill, centered above the volume
                // SLIDER (the 22px left pad matches the speaker icon + spacing).
                HorizontalLayout {
                    height: 30px;
                    Rectangle { width: 22px; background: transparent; }
                    Rectangle { horizontal-stretch: 1; background: transparent; }
                    Rectangle {
                        width: 92px; height: 30px;
                        border-radius: 15px;
                        background: root.focus-enabled
                            ? (mbf-ta.pressed ? Palette.clay-bright : Palette.clay)
                            : (mbf-ta.pressed ? Palette.dock-hi : (mbf-ta.has-hover ? Palette.control-hi : Palette.control-face));
                        border-width: 1px;
                        border-color: root.focus-enabled ? Palette.clay-bright : Palette.dock-line;
                        Text {
                            width: parent.width; height: parent.height;
                            text: "FOCUS";
                            color: root.focus-enabled ? Palette.sheen : Palette.dock-text;
                            font-size: 10px; font-weight: 700; letter-spacing: 0.6px;
                            horizontal-alignment: center; vertical-alignment: center;
                        }
                        mbf-ta := TouchArea {
                            clicked => { root.focus-enabled = !root.focus-enabled; root.focus-toggle(); }
                        }
                    }
                    Rectangle { horizontal-stretch: 1; background: transparent; }
                }
                // Volume â€” speaker + slider.
                HorizontalLayout {
                    height: 18px; spacing: 8px;
                    Rectangle {
                        width: 14px; height: parent.height; background: transparent;
                        Glyph {
                            width: 15px; height: 15px;
                            x: (parent.width - self.width)/2; y: (parent.height - self.height)/2;
                            d: Icons.speaker; tint: Palette.dock-dim;
                        }
                    }
                    WarmSlider {
                        horizontal-stretch: 1; height: 18px;
                        minimum: 0; maximum: 100;
                        value: root.volume;
                        changed(v) => { root.volume = v; root.volume-changed(v); }
                    }
                }
            }
            // Layout cycle â€” the way out of Mini (the header icon is hidden).
            Rectangle {
                width: 30px; height: 30px;
                y: (parent.height - self.height) / 2;
                border-radius: 8px;
                background: mbl-ta.has-hover ? Palette.control-hi : transparent;
                Glyph {
                    width: 18px; height: 18px;
                    x: (parent.width - self.width) / 2;
                    y: (parent.height - self.height) / 2;
                    d: Icons.layout;
                    tint: Palette.dock-text;
                    outline: true; sw: 1.3px;
                }
                mbl-ta := TouchArea { clicked => { root.cycle-layout(); } }
            }
        }
    }

    // Playlists list card, extracted so it can be instantiated two ways: a
    // a vertical-stretch fill matching the queue / downloads list.
    // Binding height to parent inside a
    // layout loops, so the fill must come from `vertical-stretch`, not `height`.
    component PlaylistCard inherits Rectangle {
        in property <[QueueRow]> items;
        callback pick(string);
        background: transparent;
        Rectangle {
            x: 2px; y: 6px; width: parent.width - 4px; height: parent.height - 6px;
            border-radius: 10px;
            background: Palette.shadow;
        }
        Rectangle {
            x: 0px; y: 0px; width: parent.width; height: parent.height - 4px;
            background: @linear-gradient(180deg, Palette.surface-lift 0%, Palette.surface 72%);
            border-radius: 10px;
            border-width: 1px; border-color: Palette.hairline;
        }
        Rectangle {
            x: 1px; y: 1px; width: parent.width - 2px; height: 22px;
            border-radius: 9px;
            background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.30) 0%, transparent 100%);
        }
        if root.items.length == 0: VerticalLayout {
            x: 34px; width: parent.width - 68px; height: parent.height;
            alignment: center; spacing: 7px;
            Text {
                text: "No saved playlists";
                color: Palette.text;
                font-family: "Segoe UI";
                font-weight: 650;
                font-size: 14px; horizontal-alignment: center;
            }
            Text {
                text: "Build a queue, then save it here.";
                color: Palette.text-faint; font-size: 11px; letter-spacing: 0px;
                horizontal-alignment: center; wrap: word-wrap;
            }
        }
        Flickable {
            x: 8px; y: 8px; width: parent.width - 16px; height: parent.height - 16px;
            viewport-width: self.width;
            viewport-height: root.items.length * 41px;
            VerticalLayout {
                width: parent.width; spacing: 5px;
                for row in root.items: Rectangle {
                    height: 36px;
                    background: pl-row-ta.has-hover ? Palette.surface-hi : transparent;
                    border-radius: 8px;
                    Text {
                        x: 12px; width: parent.width - 20px; height: parent.height;
                        text: row.title;
                        color: Palette.text;
                        font-family: "Segoe UI";
                        font-size: 13px; font-weight: 600; letter-spacing: 0px;
                        vertical-alignment: center; overflow: elide;
                    }
                    pl-row-ta := TouchArea {
                        x: 0; width: parent.width; height: parent.height;
                        clicked => { root.pick(row.url); }
                    }
                }
            }
        }
    }

    export component AppWindow inherits Window {
        // v4: the window is freely resizable. Content areas (queue / playlists /
        // downloads) absorb extra height via vertical-stretch; the transport,
        // header, and top bar stay fixed.
        preferred-width: 492px;
        // The TUNE popover grows the window (its height is added to min/preferred
        // so the OS resizes up) rather than stealing height from the track list.
        // Settings is NOT here â€” it floats as an overlay and never resizes the
        // window (see the settings overlay near the root, below the header).
        property <length> popover-extra:
            (root.tune-open ? 204px : 0px);
        // Phase 2b: 0 = Standard (resizable), 1 = Mini (fixed thin strip).
        // v6.2: min-height guarantees the track list can show at least 6 rows
        // (the output bar was removed and the transport slimmed, freeing space).
        preferred-height: root.layout-mode == 1 ? 108px : (808px + root.popover-extra);
        min-height: root.layout-mode == 1 ? 108px : (640px + root.popover-extra);
        min-width: 452px;
        title: "SideTone";
        icon: @image-url("../assets/sidetone-logo.png");

        in-out property <string> source-mode: "youtube";
        in-out property <string> input-text: "";
        in-out property <string> status-text: "Ready";
        in-out property <string> now-title: "Nothing playing";
        in-out property <int> theme-mode: 0;
        in-out property <[QueueRow]> queue-items;
        in-out property <float> progress-percent: 0;
        in-out property <string> progress-text: "0:00 / 0:00";
        in-out property <bool> progress-seekable: false;
        in-out property <float> volume: 64;
        in-out property <bool> playback-active: false;
        in-out property <bool> playback-paused: false;
        in-out property <bool> focus-enabled: false;
        in-out property <float> focus-intensity: 50;
        // 0 = off, 1 = repeat-all, 2 = repeat-one
        in-out property <int> repeat-mode: 0;
        // Shuffle toggle: when on, the visible list is randomized; turning it off
        // restores the original order (backup held in Rust).
        in-out property <bool> shuffle-active: false;
        // Phase 2b: 0 = Standard, 1 = Mini. Drives which shell the window
        // renders. Persisted to layout.json.
        in-out property <int> layout-mode: 0;
        // Local tab: 0 = Library, 1 = Playlists, 2 = Downloads.
        in-out property <int> local-tab: 0;
        // Legacy bool kept in sync for older view logic and Rust helpers.
        in-out property <bool> local-show-playlists: false;
        in-out property <bool> output-expanded: false;
        // Settings â†’ Output card: dropdown expand state.
        in-out property <bool> output-dropdown-open: false;
        in-out property <string> output-label: "Default";
        in-out property <[OutputRow]> output-items;
        in-out property <string> streaming-helper-label: "Streaming helpers: checking...";
        in-out property <string> streaming-helper-action: "Check";
        in-out property <bool> status-flash: false;
        in-out property <bool> import-active: false;

        // Update check: a banner appears when a newer release is published.
        in-out property <bool> update-available: false;
        in-out property <string> update-label: "A new version is available";
        callback open-update();
        callback dismiss-update();

        // Hotkey settings (in-app rebind UI).
        in-out property <bool> settings-open: false;
        // Which settings tab is showing: 0 = Streaming, 1 = Shortcuts, 2 = Output.
        in-out property <int> settings-tab: 0;
        in-out property <[HotkeyRow]> hotkey-items;
        // Non-empty when hotkeys are unavailable or a combo failed to register.
        in-out property <string> hotkey-status: "";
        // Index of the hotkey row currently waiting for a keypress (-1 = none).
        in-out property <int> hotkey-capturing: -1;
        // Snapshot of the combo being held during capture; committed on release.
        private property <bool> cap-ctrl;
        private property <bool> cap-alt;
        private property <bool> cap-shift;
        private property <bool> cap-meta;
        private property <string> cap-key;
        private property <bool> cap-armed;

        // TUNE dial (speed + reverb). speed is a percent (50â€“150, 100 = normal);
        // reverb is a percent (0â€“100). tune-active drives the transport indicator.
        in-out property <bool> tune-open: false;
        in-out property <float> tune-speed: 100;
        in-out property <float> tune-reverb: 0;
        property <bool> tune-active: root.tune-speed != 100 || root.tune-reverb > 0;
        // Per-track tune memory is offered only for local/downloaded tracks.
        in-out property <bool> tune-can-save: false;
        in-out property <bool> tune-saved: false;

        in-out property <[QueueRow]> search-results;

        // Playlists
        in-out property <[QueueRow]> playlist-items;
        // What the lower Local section currently shows: "DOWNLOADS" or a
        // playlist name. Keeps opened playlists from masquerading as downloads.
        in-out property <string> local-list-label: "LIBRARY";
        in-out property <bool> playlist-dropdown-open: false;
        in-out property <bool> naming-playlist: false;
        in-out property <bool> naming-whole-queue: false;
        in-out property <bool> name-flash: false;
        // How many rows in the current list are checkbox-selected.
        in-out property <int> selected-count: 0;
        // Delete confirmation dialog.
        in-out property <bool> confirm-open: false;
        in-out property <string> confirm-message: "";
        in-out property <string> pending-action: "";   // "download" / "playlist" / "selection"
        in-out property <string> pending-url: "";
        // Add-to-playlist picker (create new vs. add to a saved one).
        in-out property <bool> picker-open: false;

        callback create-playlist();
        callback create-queue-playlist();
        callback save-playlist(string);
        callback cancel-naming();
        callback local-activate();
        callback playlist-open(string);
        callback playlist-delete(string);
        callback play-queue-row(string);
        callback toggle-select(string);
        callback delete-selection();
        callback download-selection();
        callback open-playlist-picker();
        callback add-to-playlist(string);
        callback open-downloads-folder();

        callback youtube-refresh();
        callback youtube-submit(string);
        callback youtube-add-to-queue(string);
        callback youtube-queue-result(string, string);
        callback youtube-play-result(string);
        callback import-cancel();
        callback local-scan(string);
        callback local-filter(string);
        callback local-library();
        callback local-playlists();
        callback local-favorites();
        callback pause-toggle();
        callback stop-playback();
        callback shuffle-queue();
        callback queue-prev();
        callback queue-next();
        callback seek-changed(float);
        callback volume-changed(float);
        callback repeat-cycle();
        // Phase 2b: toggle Standard <-> Mini, or jump to one.
        callback cycle-layout();
        callback set-layout(int);
        callback output-select(int);
        callback refresh-helper-status();
        callback repair-helper();
        callback focus-toggle();
        callback focus-intensity-changed(float);
        callback save-track(string);
        callback queue-remove(string);
        callback queue-clear();
        callback theme-changed(int);
        // Hotkey rebinding: (action-index, ctrl, alt, shift, meta, key-text).
        callback hotkey-captured(int, bool, bool, bool, bool, string);
        callback hotkey-reset();
        // The progress-slider re-push now lives inside the Transport component
        // (it owns the slider); the two-way `progress-percent` binding carries
        // timer/reset updates into it.

        // TUNE: (speed-factor, reverb-fraction), e.g. (0.85, 0.35).
        callback tune-changed(float, float);
        // Per-track tune memory (local tracks only).
        callback tune-save();
        callback tune-clear();
        // Popover panels are part of the main layout, so the native window must
        // grow/shrink when they open or close instead of compressing the lists.
        callback panel-layout-changed();

        background: transparent;

        // Window shell â€” sandstone. Left square; Windows rounds the outer frame
        // itself, so we avoid a bulky second rounded box.
        Rectangle {
            width: 100%;
            height: 100%;
            background: Palette.window;
        }

        // Standard tree; Mini (mode 1) swaps it for the MiniBar.
        if root.layout-mode != 1: VerticalLayout {
            padding: 20px;
            spacing: 8px;

            // â”€â”€ Header: wordmark + tabs (fixed) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
            Rectangle {
                height: 32px;
                background: transparent;
                // Wordmark: "Side" clay + "T" moss + equalizer-bar "o" + "ne"
                // moss â€” the in-app logo. See logo reference.
                HorizontalLayout {
                    x: 0px; y: logo-ta.pressed ? 1px : 0px;
                    width: 230px; height: parent.height;
                    alignment: start; spacing: 0px;
                    opacity: logo-ta.pressed ? 0.82 : 1.0;
                    animate y, opacity { duration: 90ms; easing: ease-out; }
                    Text {
                        text: "Side";
                        color: Palette.logo-a;
                        font-family: "Inter"; font-size: 20px; font-weight: 800;
                        letter-spacing: -0.3px;
                        vertical-alignment: center;
                    }
                    Text {
                        text: "T";
                        color: Palette.logo-b;
                        font-family: "Inter"; font-size: 20px; font-weight: 800;
                        letter-spacing: -0.3px;
                        vertical-alignment: center;
                    }
                    // equalizer "o" â€” one centered wave, medium height. Extra
                    // right pad so it sits evenly between the T and the n.
                    Rectangle {
                        min-width: 25px; max-width: 25px; height: parent.height;
                        for h[i] in [6, 11, 16, 11, 6]: Rectangle {
                            x: i * 3.5px + 4px;
                            width: 2.5px; height: h * 1px;
                            y: (parent.height - self.height) / 2;
                            border-radius: 1.25px;
                            background: Palette.logo-b;
                        }
                    }
                    Text {
                        text: "ne";
                        color: Palette.logo-b;
                        font-family: "Inter"; font-size: 20px; font-weight: 800;
                        letter-spacing: -0.3px;
                        vertical-alignment: center;
                    }
                }
                // Click the wordmark to cycle the color theme (persisted to
                // theme.json). Default is theme 1; the last-used theme is restored
                // on the next launch.
                Rectangle {
                    x: 0px; y: 0px; width: 150px; height: parent.height;
                    background: transparent;
                    logo-ta := TouchArea {
                        clicked => {
                            root.theme-mode = root.theme-mode >= 3 ? 0 : root.theme-mode + 1;
                            Palette.theme = root.theme-mode;
                            root.theme-changed(root.theme-mode);
                        }
                    }
                }
                Rectangle {
                    x: parent.width - 120px; y: 3px;
                    width: 62px; height: 26px;
                    background: transparent;
                    Text {
                        width: parent.width; height: parent.height;
                        text: "Stream";
                        color: root.source-mode == "youtube" ? Palette.text : Palette.text-dim;
                        font-family: "Inter";
                        font-size: 12px;
                        font-weight: root.source-mode == "youtube" ? 700 : 500;
                        letter-spacing: 0.2px;
                        horizontal-alignment: center; vertical-alignment: center;
                    }
                    if root.source-mode == "youtube": Rectangle {
                        x: 12px; y: parent.height - 4px;
                        width: parent.width - 24px; height: 2px;
                        border-radius: 1px;
                        background: Palette.clay.with-alpha(0.48);
                    }
                    TouchArea {
                        clicked => { root.shuffle-active = false; root.source-mode = "youtube"; root.input-text = ""; root.youtube-refresh(); }
                    }
                }
                Rectangle {
                    x: parent.width - 52px; y: 3px;
                    width: 52px; height: 26px;
                    background: transparent;
                    Text {
                        width: parent.width; height: parent.height;
                        text: "Local";
                        color: root.source-mode == "local" ? Palette.text : Palette.text-dim;
                        font-family: "Inter";
                        font-size: 12px;
                        font-weight: root.source-mode == "local" ? 700 : 500;
                        letter-spacing: 0.2px;
                        horizontal-alignment: center; vertical-alignment: center;
                    }
                    if root.source-mode == "local": Rectangle {
                        x: 10px; y: parent.height - 4px;
                        width: parent.width - 20px; height: 2px;
                        border-radius: 1px;
                        background: Palette.clay.with-alpha(0.48);
                    }
                    TouchArea {
                        clicked => { root.shuffle-active = false; root.source-mode = "local"; root.input-text = ""; root.search-results = []; root.local-activate(); }
                    }
                }
                // Layout toggle: Standard <-> Mini. Sits left of the
                // tabs; tints clay once a non-Standard mode is active.
                layout-btn := Rectangle {
                    x: parent.width - 154px; y: 4px;
                    width: 24px; height: 24px;
                    border-radius: 6px;
                    background: layout-ta.has-hover ? Palette.surface.with-alpha(0.55) : transparent;
                    Glyph {
                        width: 19px; height: 19px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        d: Icons.layout;
                        tint: root.layout-mode != 0 ? Palette.clay : Palette.text-dim;
                        outline: true; sw: 1.3px;
                    }
                    layout-ta := TouchArea {
                        clicked => { root.cycle-layout(); }
                    }
                }
                // Settings: Output device + Global Hotkeys. Opens a panel above
                // the search bar (like the TUNE popover). Sits left of the
                // layout (Mini) icon.
                settings-btn := Rectangle {
                    x: parent.width - 186px; y: 4px;
                    width: 24px; height: 24px;
                    border-radius: 6px;
                    background: (settings-ta.has-hover || root.settings-open) ? Palette.surface.with-alpha(0.55) : transparent;
                    Glyph {
                        width: 18px; height: 18px;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                        d: Icons.gear;
                        tint: root.settings-open ? Palette.clay : Palette.text-dim;
                        outline: true; sw: 1.3px;
                    }
                    settings-ta := TouchArea {
                        clicked => {
                            root.settings-open = !root.settings-open;
                            if (root.settings-open) { root.tune-open = false; }
                            if (root.settings-open) { root.refresh-helper-status(); }
                            if (!root.settings-open) { root.hotkey-capturing = -1; root.output-dropdown-open = false; }
                            root.panel-layout-changed();
                        }
                    }
                }
            }

            // â”€â”€ Update banner â€” only when a newer release exists â”€â”€
            if root.update-available: Rectangle {
                height: 30px;
                border-radius: 9px;
                background: Palette.clay.with-alpha(upd-ta.has-hover ? 0.26 : 0.16);
                border-width: 1px; border-color: Palette.clay.with-alpha(0.5);
                // Bottom layer: clicking the banner opens the release page.
                upd-ta := TouchArea {
                    clicked => { root.open-update(); }
                }
                HorizontalLayout {
                    x: 12px; width: parent.width - 24px; height: parent.height;
                    spacing: 8px;
                    Text {
                        horizontal-stretch: 1;
                        text: "â†‘  " + root.update-label + " â€” click to download";
                        color: Palette.clay-bright;
                        font-size: 11px; font-weight: 600;
                        vertical-alignment: center; overflow: elide;
                    }
                    // On top so the X dismisses without triggering the banner click.
                    Rectangle {
                        width: 18px; height: parent.height; background: transparent;
                        Glyph {
                            width: 12px; height: 12px;
                            x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                            d: Icons.close; tint: x-upd-ta.has-hover ? Palette.clay-bright : Palette.text-dim;
                            outline: true; sw: 1.8px;
                        }
                        x-upd-ta := TouchArea { clicked => { root.dismiss-update(); } }
                    }
                }
            }

            // â”€â”€ Top bar: Search (YouTube) / Scan (Local) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
            // Frosted-glass accent: faint top sheen over warm surface.
            Rectangle {
                height: 36px;
                background: Palette.surface.with-alpha(0.48);
                border-radius: 8px;
                border-width: root.naming-playlist ? 2px : 1px;
                border-color: root.naming-playlist ? Palette.clay : Palette.hairline.with-alpha(0.62);
                animate border-color { duration: 350ms; easing: ease-out; }
                Rectangle {
                    x: 1px; y: 1px; width: parent.width - 2px; height: 12px;
                    border-radius: 7px;
                    background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.20) 0%, transparent 100%);
                }
                HorizontalLayout {
                    padding-left: 12px; padding-right: 6px; spacing: 6px;
                    Rectangle {
                        width: 16px; height: parent.height; background: transparent;
                        Glyph {
                            width: 15px; height: 15px;
                            x: (parent.width - self.width) / 2;
                            y: (parent.height - self.height) / 2;
                            d: Icons.search;
                            tint: root.naming-playlist ? Palette.clay : Palette.text-dim;
                            outline: true; sw: 1.7px;
                        }
                    }
                    Rectangle {
                        horizontal-stretch: 1; height: parent.height; background: transparent;
                        Text {
                            x: 2px; width: parent.width - 4px; height: parent.height;
                            text: root.naming-playlist
                                ? "Name your playlistâ€¦"
                                : (root.source-mode == "youtube"
                                    ? "Search or paste a link"
                                    : "Search local tracks");
                            color: root.naming-playlist
                                ? (root.name-flash ? Palette.clay-bright : Palette.clay)
                                : Palette.text-faint;
                            animate color { duration: 450ms; easing: ease-out; }
                            font-size: 12px;
                            font-weight: root.naming-playlist ? 600 : 400;
                            vertical-alignment: center;
                            visible: search-input.text == "";
                        }
                        search-input := TextInput {
                            x: 2px; width: parent.width - 4px; height: parent.height;
                            text: root.input-text;
                            color: Palette.text;
                            font-size: 12px;
                            vertical-alignment: center;
                            single-line: true;
                            edited => {
                                root.input-text = self.text;
                                // In Local, typing live-filters the open list
                                // (Downloads / Playlists / an opened playlist).
                                if (!root.naming-playlist && root.source-mode == "local") {
                                    root.local-filter(self.text);
                                }
                            }
                            accepted => {
                                if root.naming-playlist { root.save-playlist(self.text); }
                                else if root.source-mode == "youtube" { root.youtube-submit(self.text); }
                                else if root.local-tab == 0 { root.local-scan(self.text); }
                                else { root.local-filter(self.text); }
                            }
                        }
                    }
                    if root.naming-playlist: Rectangle {
                        width: 28px; height: parent.height; background: transparent;
                        Text { width: parent.width; height: parent.height; text: "âœ“"; color: Palette.clay; font-size: 14px; horizontal-alignment: center; vertical-alignment: center; }
                        TouchArea { clicked => { root.save-playlist(root.input-text); } }
                    }
                    if root.naming-playlist: Rectangle {
                        width: 28px; height: parent.height; background: transparent;
                        Text { width: parent.width; height: parent.height; text: "âœ•"; color: Palette.text-dim; font-size: 12px; horizontal-alignment: center; vertical-alignment: center; }
                        TouchArea { clicked => { root.cancel-naming(); } }
                    }
                    if root.source-mode == "youtube" && !root.naming-playlist: Rectangle {
                        width: 56px; height: parent.height; background: transparent;
                        Text { width: parent.width; height: parent.height; text: "Search"; color: yq-ta.has-hover ? Palette.logo-a : Palette.logo-a.with-alpha(0.86); font-size: 11px; font-weight: 600; horizontal-alignment: center; vertical-alignment: center; }
                        yq-ta := TouchArea { clicked => { root.youtube-submit(root.input-text); } }
                    }
                    if root.source-mode == "local" && !root.naming-playlist: Rectangle {
                        width: 44px; height: parent.height; background: transparent;
                        Text { width: parent.width; height: parent.height; text: root.local-tab == 0 ? "scan" : "filter"; color: sc-ta.has-hover ? Palette.clay-bright : Palette.text-dim; font-size: 11px; horizontal-alignment: center; vertical-alignment: center; }
                        sc-ta := TouchArea {
                            clicked => {
                                if root.local-tab == 0 { root.local-scan(""); }
                                else { root.local-filter(root.input-text); }
                            }
                        }
                    }
                }
            }

            // Settings (Output + Hotkeys) is no longer inline here â€” it floats as
            // a root-level overlay below, so opening it never pushes the content
            // down or grows the window. See the "Settings overlay" block near the
            // confirmation dialogs.

            // â•â•â•â• CONTENT AREA â€” absorbs extra window height â•â•â•â•â•â•

            // â”€â”€ YouTube: search results + queue â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
            // Status lives under the input so long import/progress messages have
            // the full width instead of being squeezed beside footer controls.
            Rectangle {
                height: 16px;
                background: transparent;
                Text {
                    x: 2px; y: 0px;
                    width: root.import-active ? parent.width - 74px : parent.width - 4px;
                    height: parent.height;
                    text: root.status-text;
                    color: root.status-flash ? Palette.logo-a : Palette.logo-a.with-alpha(0.86);
                    animate color { duration: 800ms; easing: ease-out; }
                    font-size: 10px; font-weight: 600;
                    vertical-alignment: center;
                    overflow: elide;
                }
                if root.import-active: Rectangle {
                    x: parent.width - 60px; y: 0px;
                    width: 58px; height: 16px;
                    border-radius: 6px;
                    background: stop-import-ta.has-hover ? Palette.surface-hi : Palette.surface;
                    border-width: 1px; border-color: Palette.hairline;
                    Text {
                        width: parent.width; height: parent.height;
                        text: "Stop";
                        color: Palette.clay;
                        font-size: 10px; font-weight: 700;
                        horizontal-alignment: center; vertical-alignment: center;
                    }
                    stop-import-ta := TouchArea { clicked => { root.import-cancel(); } }
                }
            }

            if root.source-mode == "youtube": VerticalLayout {
                vertical-stretch: 1;
                spacing: 10px;

                // Search results â€” only present while searching (capped)
                if root.search-results.length > 0: VerticalLayout {
                    height: root.search-results.length > 5 ? 200px : (root.search-results.length * 34px + 26px);
                    spacing: 6px;
                    Text {
                        text: "RESULTS";
                        color: Palette.text-dim;
                        font-size: 9px; font-weight: 700; letter-spacing: 1px;
                        height: 12px; vertical-alignment: center;
                    }
                    Flickable {
                        vertical-stretch: 1;
                        viewport-width: self.width;
                        viewport-height: root.search-results.length * 34px;
                        VerticalLayout {
                            width: parent.width; spacing: 4px;
                            for row in root.search-results: Rectangle {
                                height: 30px;
                                background: sr-ta.has-hover ? Palette.surface-hi : Palette.surface;
                                border-radius: 8px;
                                border-width: 1px;
                                border-color: sr-ta.has-hover ? Palette.hairline-hi : Palette.hairline;
                                Text {
                                    x: 11px; width: parent.width - 64px; height: parent.height;
                                    text: row.title;
                                    color: sr-ta.has-hover ? Palette.text : Palette.text-dim;
                                    font-size: 11px; vertical-alignment: center; overflow: elide;
                                }
                                sr-ta := TouchArea {
                                    x: 0; width: parent.width - 54px; height: parent.height;
                                    clicked => { root.youtube-play-result(row.url); }
                                }
                                Rectangle {
                                    x: parent.width - 52px; width: 24px; height: parent.height; background: transparent;
                                    Text { width: parent.width; height: parent.height; text: "+Q"; color: Palette.text-dim; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                                    TouchArea { clicked => { root.youtube-queue-result(row.url, row.title); } }
                                }
                                Rectangle {
                                    x: parent.width - 24px; width: 22px; height: parent.height; background: transparent;
                                    Text { width: parent.width; height: parent.height; text: "â†“"; color: Palette.text-dim; font-size: 13px; horizontal-alignment: center; vertical-alignment: center; }
                                    TouchArea { clicked => { root.save-track(row.url); } }
                                }
                            }
                        }
                    }
                }

                // Queue â€” the primary content area
                VerticalLayout {
                    vertical-stretch: 1;
                    spacing: 6px;
                    HorizontalLayout {
                        height: 16px; spacing: 0px;
                        Text {
                            text: "QUEUE";
                            color: Palette.text-dim;
                            font-size: 9px; font-weight: 700; letter-spacing: 1px;
                            vertical-alignment: center;
                        }
                        Rectangle { width: 7px; background: transparent; }
                        Text {
                            text: root.queue-items.length > 0 ? ("Â· " + root.queue-items.length) : "";
                            color: Palette.text-faint;
                            font-size: 9px; font-weight: 700;
                            vertical-alignment: center;
                        }
                        Rectangle { width: 12px; background: transparent; }
                        if root.queue-items.length > 0 && root.selected-count == 0: Rectangle {
                            width: 92px; height: 16px; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "Save Playlist";
                                color: q-save-all-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                font-size: 11px; font-weight: 600;
                                horizontal-alignment: left; vertical-alignment: center;
                            }
                            q-save-all-ta := TouchArea { clicked => { root.create-queue-playlist(); } }
                        }
                        if root.queue-items.length > 0 && root.selected-count == 0: Rectangle {
                            width: 78px; height: 16px; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "Clear Queue";
                                color: q-clear-ta.has-hover ? Palette.clay-bright : Palette.text-dim;
                                font-size: 11px; font-weight: 600;
                                horizontal-alignment: left; vertical-alignment: center;
                            }
                            q-clear-ta := TouchArea { clicked => { root.queue-clear(); } }
                        }
                        Rectangle { horizontal-stretch: 1; background: transparent; }
                        // Hint when nothing is checked.
                        if root.selected-count == 0: Text {
                            text: root.queue-items.length > 0 ? "Select tracks to save or download" : "Select tracks to add or download";
                            color: Palette.text-faint;
                            font-size: 10px; vertical-alignment: center;
                        }
                        // Add the checked tracks to a playlist (create / existing).
                        if root.selected-count > 0: Rectangle {
                            width: 96px; height: 16px; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "+ Playlist (" + root.selected-count + ")";
                                color: q-add-pl-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                font-size: 11px; font-weight: 600; horizontal-alignment: right; vertical-alignment: center;
                            }
                            q-add-pl-ta := TouchArea { clicked => { root.open-playlist-picker(); } }
                        }
                        // Download the checked tracks to the local library.
                        if root.selected-count > 0: Rectangle {
                            width: 96px; height: 16px; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "â†“ Download (" + root.selected-count + ")";
                                color: q-dl-sel-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                font-size: 11px; font-weight: 600; horizontal-alignment: right; vertical-alignment: center;
                            }
                            q-dl-sel-ta := TouchArea { clicked => { root.download-selection(); } }
                        }
                    }
                    Rectangle {
                        vertical-stretch: 1;
                        background: transparent;
                        Rectangle {
                            x: 2px; y: 6px; width: parent.width - 4px; height: parent.height - 6px;
                            border-radius: 10px;
                            background: Palette.shadow;
                        }
                        Rectangle {
                            x: 0px; y: 0px; width: parent.width; height: parent.height - 4px;
                            background: @linear-gradient(180deg, Palette.surface-lift 0%, Palette.surface 72%);
                            border-radius: 10px;
                            border-width: 1px;
                            border-color: Palette.hairline;
                        }
                        Rectangle {
                            x: 1px; y: 1px; width: parent.width - 2px; height: 22px;
                            border-radius: 9px;
                            background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.30) 0%, transparent 100%);
                        }
                        if root.queue-items.length == 0: VerticalLayout {
                            x: 34px; width: parent.width - 68px; height: parent.height;
                            alignment: center; spacing: 7px;
                            Text {
                                text: "Ready to queue";
                                color: Palette.text;
                                font-family: "Segoe UI";
                                font-weight: 650;
                                font-size: 14px; horizontal-alignment: center;
                            }
                            Text {
                                text: "Search or paste a link above to add tracks.";
                                color: Palette.text-faint; font-size: 11px; letter-spacing: 0px;
                                horizontal-alignment: center; wrap: word-wrap;
                            }
                        }
                        Flickable {
                            x: 8px; y: 8px;
                            width: parent.width - 16px; height: parent.height - 16px;
                            viewport-width: self.width;
                            viewport-height: root.queue-items.length * 35px;
                            VerticalLayout {
                                width: parent.width; spacing: 2px;
                                for row in root.queue-items: Rectangle {
                                    height: 33px;
                                    background: row.active ? Palette.active-bg : (q-row-ta.has-hover ? Palette.surface-hi.with-alpha(0.45) : transparent);
                                    border-radius: 8px;
                                    Rectangle {
                                        x: 2px; y: 7px; width: 2px; height: parent.height - 14px;
                                        border-radius: 1px;
                                        background: row.active ? Palette.clay : transparent;
                                    }
                                    // selection checkbox â€” subtle until hovered / checked
                                    Rectangle {
                                        x: 7px; width: 22px; height: parent.height; background: transparent;
                                        Rectangle {
                                            width: 15px; height: 15px;
                                            x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                                            border-radius: 4px;
                                            background: row.selected ? Palette.clay : transparent;
                                            border-width: 1px;
                                            border-color: row.selected ? Palette.clay
                                                : (q-cb-ta.has-hover ? Palette.text-dim : Palette.text-faint.with-alpha(0.32));
                                            if row.selected: Text {
                                                width: parent.width; height: parent.height;
                                                text: "âœ“"; color: Palette.sheen;
                                                font-size: 11px; font-weight: 800;
                                                horizontal-alignment: center; vertical-alignment: center;
                                            }
                                        }
                                        q-cb-ta := TouchArea { clicked => { root.toggle-select(row.url); } }
                                    }
                                    Text {
                                        x: 33px; width: parent.width - 92px; height: parent.height;
                                        text: row.title;
                                        color: row.active ? Palette.active-text : Palette.text-dim;
                                        font-size: 12px; vertical-alignment: center; overflow: elide;
                                    }
                                    q-row-ta := TouchArea {
                                        x: 31px; width: parent.width - 90px; height: parent.height;
                                        enabled: row.url != "";
                                        clicked => { root.play-queue-row(row.url); }
                                    }
                                    if row.is-remote && !row.downloaded: Rectangle {
                                        x: parent.width - 56px; width: 24px; height: parent.height; background: transparent;
                                        Glyph {
                                            width: 16px; height: 16px;
                                            x: (parent.width - self.width)/2; y: (parent.height - self.height)/2;
                                            d: Icons.download;
                                            tint: q-dl-ta.has-hover ? Palette.clay-bright : Palette.text-dim;
                                            outline: true; sw: 1.6px;
                                        }
                                        q-dl-ta := TouchArea { clicked => { root.save-track(row.url); } }
                                    }
                                    Rectangle {
                                        x: parent.width - 28px; width: 24px; height: parent.height; background: transparent;
                                        Glyph {
                                            width: 13px; height: 13px;
                                            x: (parent.width - self.width)/2; y: (parent.height - self.height)/2;
                                            d: Icons.close;
                                            tint: q-rm-ta.has-hover ? Palette.clay-bright : Palette.text-faint;
                                            outline: true; sw: 1.8px;
                                        }
                                        q-rm-ta := TouchArea { clicked => { root.queue-remove(row.url); } }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // â”€â”€ Local: Playlists + Downloads â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
            if root.source-mode == "local": VerticalLayout {
                vertical-stretch: 1;
                spacing: 12px;

                // Local tab row to switch the single list between Playlists
                // (pick one) and Downloads (tracks).
                // Local tab switcher: Playlists | Downloads. Shown in every mode
                // (Mini gates the whole tree off, so it only renders here).
                HorizontalLayout {
                    height: 22px; spacing: 18px;
                    for tab in [
                        { label: "Library", mode: 0 },
                        { label: "Playlists", mode: 1 },
                        { label: "Downloads", mode: 2 },
                    ]: Rectangle {
                        horizontal-stretch: 0;
                        width: lt-txt.preferred-width + 4px;
                        background: transparent;
                        lt-txt := Text {
                            text: tab.label;
                            color: root.local-tab == tab.mode ? Palette.text : Palette.text-dim;
                            font-size: 11px;
                            font-weight: root.local-tab == tab.mode ? 700 : 400;
                            letter-spacing: 0.5px;
                            vertical-alignment: center;
                        }
                        if root.local-tab == tab.mode: Rectangle {
                            y: parent.height - 3px; height: 2px;
                            width: parent.width; border-radius: 1px;
                            background: Palette.clay.with-alpha(0.55);
                        }
                        TouchArea {
                            clicked => {
                                root.local-tab = tab.mode;
                                root.local-show-playlists = tab.mode == 1;
                                if (tab.mode == 0) { root.local-library(); }
                                else if (tab.mode == 1) { root.local-playlists(); }
                                else { root.local-favorites(); }
                            }
                        }
                    }
                    Rectangle { horizontal-stretch: 1; background: transparent; }
                }

                // PLAYLISTS tab: the list of playlists, shown when the Playlists
                // tab is active and no playlist is open yet. Opening one keeps the
                // Playlists tab active and shows its tracks in the section below.
                if root.local-tab == 1 && root.local-list-label == "PLAYLISTS": VerticalLayout {
                    vertical-stretch: 1;
                    spacing: 6px;
                    HorizontalLayout {
                        height: 16px;
                        Text {
                            text: "PLAYLISTS";
                            color: Palette.text-dim;
                            font-size: 9px; font-weight: 700; letter-spacing: 1px;
                            vertical-alignment: center;
                        }
                        Rectangle { width: 7px; background: transparent; }
                        Text {
                            text: root.playlist-items.length > 0 ? ("Â· " + root.playlist-items.length) : "";
                            color: Palette.text-faint;
                            font-size: 9px; font-weight: 700;
                            vertical-alignment: center;
                        }
                        Rectangle { horizontal-stretch: 1; background: transparent; }
                    }
                    PlaylistCard {
                        vertical-stretch: 1;
                        items: root.playlist-items;
                        // Stay on the Playlists tab; the tracks show below.
                        pick(u) => { root.local-tab = 1; root.local-show-playlists = true; root.playlist-open(u); }
                    }
                }

                // Track list â€” Downloads tab (downloads), or the Playlists tab
                // when a playlist is open (showing that playlist's tracks).
                if root.local-tab != 1 || root.local-list-label != "PLAYLISTS": VerticalLayout {
                    vertical-stretch: 1;
                    spacing: 6px;
                    HorizontalLayout {
                        height: 16px; spacing: 8px;
                        Text {
                            text: root.local-list-label == "LIBRARY"
                                ? "LIBRARY"
                                : (root.local-list-label == "DOWNLOADS"
                                    ? "DOWNLOADS"
                                    : ("PLAYLIST - " + root.local-list-label));
                            color: Palette.text-dim;
                            font-size: 9px; font-weight: 700; letter-spacing: 1px;
                            vertical-alignment: center; overflow: elide;
                        }
                        // Delete the whole playlist (only while one is open) â€” sits
                        // right next to the playlist title, routed through confirm.
                        if root.local-tab == 1 && root.local-list-label != "PLAYLISTS": Rectangle {
                            width: 22px; height: parent.height; background: transparent;
                            Glyph {
                                width: 14px; height: 14px;
                                x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                                d: Icons.trash; tint: pl-trash-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                outline: true; sw: 1.5px;
                            }
                            pl-trash-ta := TouchArea {
                                clicked => {
                                    root.pending-action = "playlist";
                                    root.pending-url = root.local-list-label;
                                    root.confirm-message = "Delete the playlist \"" + root.local-list-label + "\"?";
                                    root.confirm-open = true;
                                }
                            }
                        }
                        Rectangle { horizontal-stretch: 1; background: transparent; }
                        // Add checked downloads to a playlist (Downloads view only;
                        // a playlist's own tracks just get Remove).
                        if root.selected-count > 0 && (root.local-list-label == "LIBRARY" || root.local-list-label == "DOWNLOADS"): Rectangle {
                            width: 86px; height: parent.height; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "+ Playlist (" + root.selected-count + ")";
                                color: dl-add-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                font-size: 10px; font-weight: 600; horizontal-alignment: right; vertical-alignment: center;
                            }
                            dl-add-ta := TouchArea { clicked => { root.open-playlist-picker(); } }
                        }
                        // Inside a playlist: download the selected tracks
                        // (next to Remove). Downloads tab already has the folder.
                        if root.selected-count > 0 && root.local-tab == 1 && root.local-list-label != "PLAYLISTS": Rectangle {
                            width: 96px; height: parent.height; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: "â†“ Download (" + root.selected-count + ")";
                                color: pl-dl-sel-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                font-size: 10px; font-weight: 600; horizontal-alignment: right; vertical-alignment: center;
                            }
                            pl-dl-sel-ta := TouchArea { clicked => { root.download-selection(); } }
                        }
                        if root.selected-count > 0: Rectangle {
                            width: 74px; height: parent.height; background: transparent;
                            Text {
                                width: parent.width; height: parent.height;
                                text: (root.local-list-label == "DOWNLOADS" ? "Delete (" : "Remove (") + root.selected-count + ")";
                                color: dl-del-ta.has-hover ? Palette.clay-bright : Palette.text-dim;
                                font-size: 10px; font-weight: 600; horizontal-alignment: right; vertical-alignment: center;
                            }
                            dl-del-ta := TouchArea {
                                clicked => {
                                    root.pending-action = "selection";
                                    root.confirm-message = root.local-list-label == "DOWNLOADS"
                                        ? ("Delete " + root.selected-count + " download(s) from your computer?")
                                        : (root.local-list-label == "LIBRARY"
                                            ? ("Remove " + root.selected-count + " track(s) from the library index?")
                                            : ("Remove " + root.selected-count + " track(s) from this playlist?"));
                                    root.confirm-open = true;
                                }
                            }
                        }
                        // Viewing a playlist's tracks â†’ return to the playlist list
                        // (stays on the Playlists tab; Downloads has its own tab).
                        if root.local-tab == 1 && root.local-list-label != "PLAYLISTS": Rectangle {
                            width: 104px; height: parent.height; background: transparent;
                            Glyph {
                                width: 13px; height: 13px;
                                x: parent.width - 78px; y: (parent.height - self.height) / 2;
                                d: Icons.back;
                                tint: back-dl-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                outline: true; sw: 1.9px;
                            }
                            Text {
                                x: parent.width - 62px; width: 62px; height: parent.height;
                                text: "Playlists";
                                color: back-dl-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                font-size: 11px; font-weight: 600;
                                horizontal-alignment: right; vertical-alignment: center;
                            }
                            back-dl-ta := TouchArea { clicked => { root.local-playlists(); } }
                        }
                        // Open downloads folder â€” Downloads view only (not playlists).
                        if root.local-list-label == "DOWNLOADS": Rectangle {
                            width: 22px; height: parent.height; background: transparent;
                            Glyph {
                                width: 17px; height: 17px;
                                x: (parent.width - self.width)/2; y: (parent.height - self.height)/2;
                                d: Icons.folder; tint: fldr-ta.has-hover ? Palette.clay-bright : Palette.text-dim;
                                outline: true; sw: 1.6px;
                            }
                            fldr-ta := TouchArea { clicked => { root.open-downloads-folder(); } }
                        }
                    }
                    Rectangle {
                        vertical-stretch: 1;
                        background: transparent;
                        Rectangle {
                            x: 2px; y: 6px; width: parent.width - 4px; height: parent.height - 6px;
                            border-radius: 10px;
                            background: Palette.shadow;
                        }
                        Rectangle {
                            x: 0px; y: 0px; width: parent.width; height: parent.height - 4px;
                            background: @linear-gradient(180deg, Palette.surface-lift 0%, Palette.surface 72%);
                            border-radius: 10px;
                            border-width: 1px; border-color: Palette.hairline;
                        }
                        Rectangle {
                            x: 1px; y: 1px; width: parent.width - 2px; height: 22px;
                            border-radius: 9px;
                            background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.30) 0%, transparent 100%);
                        }
                        if root.queue-items.length == 0: VerticalLayout {
                            x: 34px; width: parent.width - 68px; height: parent.height;
                            alignment: center; spacing: 7px;
                            Text {
                                text: root.local-list-label == "LIBRARY"
                                    ? "No local library"
                                    : (root.local-list-label == "DOWNLOADS"
                                        ? "No saved downloads"
                                        : "This playlist is empty");
                                color: Palette.text;
                                font-family: "Segoe UI";
                                font-weight: 650;
                                font-size: 14px; horizontal-alignment: center;
                            }
                            Text {
                                text: root.local-list-label == "LIBRARY"
                                    ? "Scan your Music folder or paste a folder path."
                                    : (root.local-list-label == "DOWNLOADS"
                                        ? "Use download on a track to keep it local."
                                        : "Add tracks from Stream, Library, or Downloads.");
                                color: Palette.text-faint; font-size: 11px; letter-spacing: 0px;
                                horizontal-alignment: center; wrap: word-wrap;
                            }
                        }
                        Flickable {
                            x: 8px; y: 8px; width: parent.width - 16px; height: parent.height - 16px;
                            viewport-width: self.width;
                            viewport-height: root.queue-items.length * 35px;
                            VerticalLayout {
                                width: parent.width; spacing: 2px;
                                for row in root.queue-items: Rectangle {
                                    height: 33px;
                                    background: row.active ? Palette.active-bg : (dl-row-ta.has-hover ? Palette.surface-hi.with-alpha(0.45) : transparent);
                                    border-radius: 8px;
                                    Rectangle {
                                        x: 2px; y: 7px; width: 2px; height: parent.height - 14px;
                                        border-radius: 1px;
                                        background: row.active ? Palette.clay : transparent;
                                    }
                                    // selection checkbox â€” subtle until hovered / checked
                                    Rectangle {
                                        x: 7px; width: 22px; height: parent.height; background: transparent;
                                        Rectangle {
                                            width: 15px; height: 15px;
                                            x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                                            border-radius: 4px;
                                            background: row.selected ? Palette.clay : transparent;
                                            border-width: 1px;
                                            border-color: row.selected ? Palette.clay
                                                : (dl-cb-ta.has-hover ? Palette.text-dim : Palette.text-faint.with-alpha(0.32));
                                            if row.selected: Text {
                                                width: parent.width; height: parent.height;
                                                text: "âœ“"; color: Palette.sheen;
                                                font-size: 11px; font-weight: 800;
                                                horizontal-alignment: center; vertical-alignment: center;
                                            }
                                        }
                                        dl-cb-ta := TouchArea { clicked => { root.toggle-select(row.url); } }
                                    }
                                    Text {
                                        x: 33px; width: parent.width - 44px; height: parent.height;
                                        text: row.title;
                                        color: row.active ? Palette.active-text : Palette.text-dim;
                                        font-size: 12px; vertical-alignment: center; overflow: elide;
                                    }
                                    dl-row-ta := TouchArea {
                                        x: 31px; width: parent.width - 40px; height: parent.height;
                                        enabled: row.url != "";
                                        clicked => { root.play-queue-row(row.url); }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // â•â•â•â• PERSISTENT TRANSPORT â€” always visible, fixed â•â•â•â•
            // Extracted into the reusable `Transport` component (Phase 2b) so
            // each layout mode can host it. Bound to AppWindow state below.
            transport := Transport {
                now-title: root.now-title;
                progress-text: root.progress-text;
                progress-percent <=> root.progress-percent;
                progress-seekable: root.progress-seekable;
                playback-active: root.playback-active;
                playback-paused: root.playback-paused;
                repeat-mode: root.repeat-mode;
                shuffle-active: root.shuffle-active;
                volume <=> root.volume;
                tune-active: root.tune-active;
                tune-open <=> root.tune-open;
                focus-enabled <=> root.focus-enabled;
                focus-intensity <=> root.focus-intensity;
                seek-changed(v) => { root.seek-changed(v); }
                queue-prev => { root.queue-prev(); }
                queue-next => { root.queue-next(); }
                pause-toggle => { root.pause-toggle(); }
                shuffle-queue => { root.shuffle-queue(); }
                repeat-cycle => { root.repeat-cycle(); }
                volume-changed(v) => { root.volume-changed(v); }
                panel-layout-changed => {
                    if (root.tune-open) { root.settings-open = false; }
                    root.panel-layout-changed();
                }
                focus-toggle => { root.focus-toggle(); }
                focus-intensity-changed(v) => { root.focus-intensity-changed(v); }
            }
            // TUNE: speed + reverb (opens below the transport so the TUNE button stays still).
            if root.tune-open: Rectangle {
                height: 188px;
                background: Palette.surface;
                border-radius: 13px;
                border-width: 1px; border-color: Palette.hairline;
                Rectangle {
                    x: 2px; y: 2px; width: parent.width - 4px; height: 24px;
                    border-radius: 12px;
                    background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.5) 0%, transparent 100%);
                }
                HorizontalLayout {
                    x: 16px; y: 14px; width: parent.width - 32px; height: parent.height - 28px;
                    spacing: 14px;
                    // LEFT â€” title, speed, presets, save
                    VerticalLayout {
                        horizontal-stretch: 1;
                        spacing: 9px;
                        Text {
                            text: "TUNE";
                            color: Palette.clay;
                            font-size: 13px; font-weight: 800; letter-spacing: 1px;
                            font-family: "Segoe UI";
                        }
                        // Speed
                        VerticalLayout {
                            spacing: 4px;
                            HorizontalLayout {
                                Text {
                                    text: "Speed"; color: Palette.text-dim;
                                    font-size: 11px; vertical-alignment: center;
                                }
                                Rectangle { horizontal-stretch: 1; background: transparent; }
                                Text {
                                    text: Math.round(root.tune-speed) + "%";
                                    color: Palette.text; font-size: 11px; font-weight: 700;
                                    vertical-alignment: center;
                                }
                            }
                            speed-slider := WarmSlider {
                                height: 20px;
                                minimum: 50; maximum: 150;
                                value: root.tune-speed;
                                changed(v) => {
                                    root.tune-speed = v;
                                    root.tune-changed(root.tune-speed / 100.0, root.tune-reverb / 100.0);
                                }
                            }
                        }
                        HorizontalLayout {
                            height: 18px; spacing: 6px;
                            Rectangle { horizontal-stretch: 1; background: transparent; }
                            Rectangle {
                                width: tune-reset-label.preferred-width + 18px; height: 18px;
                                background: transparent;
                                Glyph {
                                    width: 11px; height: 11px;
                                    x: 0px; y: (parent.height - self.height) / 2;
                                    d: Icons.repeat;
                                    tint: reset-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                    outline: true; sw: 1.7px;
                                }
                                tune-reset-label := Text {
                                    x: 14px; width: parent.width - 14px; height: parent.height;
                                    text: "Reset";
                                    color: reset-ta.has-hover ? Palette.clay-bright : Palette.clay;
                                    font-size: 11px; font-weight: 400;
                                    vertical-alignment: center;
                                }
                                reset-ta := TouchArea {
                                    clicked => {
                                        speed-slider.value = 100.0;
                                        reverb-knob.value = 0.0;
                                        root.tune-speed = 100.0;
                                        root.tune-reverb = 0.0;
                                        root.tune-changed(1.0, 0.0);
                                    }
                                }
                            }
                            Rectangle { horizontal-stretch: 1; background: transparent; }
                        }
                        // presets
                        HorizontalLayout {
                            height: 32px; spacing: 8px;
                            Rectangle { horizontal-stretch: 1; background: transparent; }
                            for preset in [
                                { label: "Slowed", spd: 85.0, rev: 0.0, wide: false },
                                { label: "Slowed + Reverb", spd: 85.0, rev: 25.0, wide: true },
                            ]: preset-pill := Rectangle {
                                property <bool> is-reverb: preset.rev > 0.5;
                                property <bool> speed-changed: Math.abs(root.tune-speed - 100.0) >= 0.6;
                                property <bool> reverb-changed: root.tune-reverb >= 0.6;
                                property <bool> preset-active:
                                    (!self.is-reverb && self.speed-changed && !self.reverb-changed)
                                    || (self.is-reverb && self.speed-changed && self.reverb-changed);
                                width: preset.wide ? 154px : 92px;
                                height: 32px;
                                Rectangle {
                                    x: 0; y: preset-ta.pressed ? 0.5px : 2.5px;
                                    width: parent.width; height: 30px;
                                    border-radius: 15px; background: Palette.control-shadow;
                                    animate y { duration: 70ms; easing: ease-out; }
                                }
                                Rectangle {
                                    width: parent.width; height: 30px;
                                    y: preset-ta.pressed ? 1.5px : 0px;
                                    border-radius: 15px;
                                    animate y { duration: 70ms; easing: ease-out; }
                                    background: parent.preset-active
                                        ? (preset-ta.pressed ? Palette.clay-bright : Palette.clay)
                                        : (preset-ta.pressed ? Palette.dock-hi : (preset-ta.has-hover ? Palette.control-hi : Palette.control-face));
                                    border-width: 1px;
                                    border-color: parent.preset-active ? Palette.clay-bright : Palette.dock-line;
                                    Rectangle {
                                        x: 1px; y: 1px; width: parent.width - 2px; height: 28px;
                                        border-radius: 14px;
                                        background: @linear-gradient(180deg, Palette.sheen.with-alpha(preset-pill.preset-active ? 0.30 : 0.50) 0%, transparent 54%);
                                    }
                                    Text {
                                        width: parent.width; height: parent.height;
                                        text: preset.label;
                                        color: preset-pill.preset-active ? Palette.sheen : Palette.dock-text;
                                        font-size: 11px; font-weight: 500;
                                        horizontal-alignment: center; vertical-alignment: center; overflow: elide;
                                    }
                                }
                                preset-ta := TouchArea {
                                    width: parent.width; height: 30px;
                                    clicked => {
                                        // Imperative set: Slint severs the value binding
                                        // after a drag, so push to the widgets directly.
                                        speed-slider.value = preset.spd;
                                        reverb-knob.value = preset.rev;
                                        root.tune-speed = preset.spd;
                                        root.tune-reverb = preset.rev;
                                        root.tune-changed(root.tune-speed / 100.0, root.tune-reverb / 100.0);
                                    }
                                }
                            }
                            Rectangle { horizontal-stretch: 1; background: transparent; }
                        }
                        // per-track memory (local tracks only)
                        HorizontalLayout {
                            height: 22px; spacing: 6px;
                            if root.tune-can-save: Rectangle {
                                horizontal-stretch: 1; height: 22px;
                                border-radius: 8px;
                                background: save-ta.has-hover ? Palette.surface-hi : Palette.active-bg.with-alpha(0.6);
                                border-width: 1px;
                                border-color: root.tune-saved ? Palette.moss : Palette.hairline;
                                Text {
                                    width: parent.width; height: parent.height;
                                    text: root.tune-saved ? "âœ“ Saved â€” update" : "Save tune for this track";
                                    color: root.tune-saved ? Palette.moss : Palette.text-dim;
                                    font-size: 10px; font-weight: 600;
                                    horizontal-alignment: center; vertical-alignment: center; overflow: elide;
                                }
                                save-ta := TouchArea { clicked => { root.tune-save(); } }
                            }
                            if root.tune-can-save && root.tune-saved: Rectangle {
                                width: 52px; height: 22px;
                                border-radius: 8px;
                                background: clear-ta.has-hover ? Palette.surface-hi : transparent;
                                Text {
                                    width: parent.width; height: parent.height;
                                    text: "Forget";
                                    color: Palette.clay; font-size: 10px; font-weight: 600;
                                    horizontal-alignment: center; vertical-alignment: center;
                                }
                                clear-ta := TouchArea { clicked => { root.tune-clear(); } }
                            }
                            if !root.tune-can-save: Text {
                                horizontal-stretch: 1; height: 22px;
                                text: "Saving is available for downloaded tracks.";
                                color: Palette.text-faint; font-size: 9px;
                                vertical-alignment: center; horizontal-alignment: center;
                            }
                        }
                    }
                    // RIGHT â€” reverb knob
                    VerticalLayout {
                        width: 146px; spacing: 2px;
                        Rectangle { height: 12px; background: transparent; }
                        Text {
                            height: 14px;
                            text: "Reverb  " + Math.round(root.tune-reverb) + "%";
                            color: Palette.moss;
                            font-size: 11px; font-weight: 700;
                            horizontal-alignment: center; vertical-alignment: center;
                        }
                        reverb-knob := Knob {
                            height: 128px;
                            minimum: 0; maximum: 100;
                            value: root.tune-reverb;
                            changed(v) => {
                                root.tune-reverb = v;
                                root.tune-changed(root.tune-speed / 100.0, root.tune-reverb / 100.0);
                            }
                        }
                    }
                }
            }
        }

        // Mini layout â€” the thin control strip (mode 1).
        if root.layout-mode == 1: MiniBar {
            width: 100%; height: 100%;
            now-title: root.now-title;
            progress-text: root.progress-text;
            playback-active: root.playback-active;
            playback-paused: root.playback-paused;
            volume <=> root.volume;
            focus-enabled <=> root.focus-enabled;
            queue-prev => { root.queue-prev(); }
            queue-next => { root.queue-next(); }
            pause-toggle => { root.pause-toggle(); }
            focus-toggle => { root.focus-toggle(); }
            volume-changed(v) => { root.volume-changed(v); }
            cycle-layout => { root.cycle-layout(); }
        }

        // Focus Mode = "evening mode": a warm walnut veil settles over the WHOLE
        // interface, dimming the centre and deepening toward the edges. It tints
        // and lowers contrast without going grey â€” like dusk in a coffee shop.
        Rectangle {
            width: 100%;
            height: 100%;
            background: @radial-gradient(circle, Palette.dim.with-alpha(0.55) 0%, Palette.dim 125%);
            opacity: root.focus-enabled ? 0.34 : 0.0;
            animate opacity { duration: 650ms; easing: ease-out; }
        }

        // â”€â”€ Settings overlay â€” one panel, sidebar tabs (macOS-style) â”€â”€
        // Opened from the header gear. A root-level overlay floating ABOVE the
        // content, anchored under the header (never resizes/pushes the UI). The
        // left rail switches tabs; the right pane swaps content. Click the scrim
        // outside the panel to dismiss.
        if root.settings-open && root.layout-mode != 1: Rectangle {
            width: 100%; height: 100%;
            background: Palette.dim.with-alpha(0.28);
            TouchArea {
                clicked => {
                    root.settings-open = false;
                    root.hotkey-capturing = -1;
                }
            }
            Rectangle {
                x: 20px; y: 60px;
                width: parent.width - 40px;
                height: 312px;
                background: Palette.surface;
                border-radius: 14px;
                border-width: 1px; border-color: Palette.hairline;
                Rectangle {
                    x: 2px; y: 2px; width: parent.width - 4px; height: 24px;
                    border-radius: 13px;
                    background: @linear-gradient(180deg, Palette.sheen.with-alpha(0.42) 0%, transparent 100%);
                }
                // Swallow clicks inside the panel so they don't dismiss it.
                TouchArea {}
                HorizontalLayout {
                    padding: 12px; spacing: 12px;
                    // â”€â”€ LEFT RAIL: tab list â”€â”€
                    VerticalLayout {
                        width: 100px; spacing: 4px;
                        Text {
                            height: 16px;
                            text: "SETTINGS";
                            color: Palette.text-faint;
                            font-size: 8px; font-weight: 700; letter-spacing: 1.5px;
                            vertical-alignment: center;
                        }
                        for tab[i] in [{ t: "Streaming" }, { t: "Shortcuts" }, { t: "Output" }]: Rectangle {
                            height: 32px;
                            border-radius: 8px;
                            background: root.settings-tab == i ? Palette.active-bg : (tab-ta.has-hover ? Palette.surface-hi : transparent);
                            Rectangle {
                                x: 5px; y: (parent.height - 14px) / 2; width: 2px; height: 14px; border-radius: 1px;
                                background: root.settings-tab == i ? Palette.clay : transparent;
                            }
                            Text {
                                x: 14px; width: parent.width - 18px; height: parent.height;
                                text: tab.t;
                                color: root.settings-tab == i ? Palette.clay : Palette.text-dim;
                                font-size: 12px; font-weight: root.settings-tab == i ? 700 : 500;
                                vertical-alignment: center;
                            }
                            tab-ta := TouchArea {
                                clicked => { root.settings-tab = i; root.hotkey-capturing = -1; }
                            }
                        }
                        Rectangle { vertical-stretch: 1; background: transparent; }
                    }
                    // divider
                    Rectangle { width: 1px; background: Palette.hairline; }
                    // â”€â”€ RIGHT PANE: content for the active tab â”€â”€
                    Rectangle {
                        horizontal-stretch: 1;

                        // â”€â”€ TAB 0 â€” Streaming (status, repair, tips) â”€â”€
                        if root.settings-tab == 0: VerticalLayout {
                            spacing: 9px;
                            Text {
                                height: 16px; text: "Streaming engine";
                                color: Palette.text; font-size: 13px; font-weight: 700;
                                vertical-alignment: center;
                            }
                            Rectangle {
                                height: 42px;
                                background: Palette.window.with-alpha(0.45);
                                border-radius: 9px;
                                border-width: 1px; border-color: Palette.hairline.with-alpha(0.6);
                                Text {
                                    x: 12px; width: parent.width - 94px; height: parent.height;
                                    text: root.streaming-helper-label;
                                    color: Palette.text-dim; font-size: 10px; font-weight: 500;
                                    vertical-alignment: center; wrap: word-wrap;
                                }
                                Rectangle {
                                    x: parent.width - 80px; y: 10px; width: 68px; height: 22px;
                                    border-radius: 8px;
                                    border-width: 1px;
                                    border-color: helper-action-ta.has-hover ? Palette.clay.with-alpha(0.70) : Palette.hairline.with-alpha(0.7);
                                    background: helper-action-ta.has-hover ? Palette.surface-hi : Palette.surface;
                                    Text {
                                        width: parent.width; height: parent.height;
                                        text: root.streaming-helper-action;
                                        color: helper-action-ta.has-hover ? Palette.clay-bright : Palette.logo-a;
                                        font-size: 10px; font-weight: 700;
                                        horizontal-alignment: center; vertical-alignment: center;
                                    }
                                    helper-action-ta := TouchArea {
                                        clicked => {
                                            if (root.streaming-helper-action == "Repairing") {
                                                // no-op while a repair is already in flight
                                            } else if (root.streaming-helper-action == "Check") {
                                                root.refresh-helper-status();
                                            } else {
                                                root.repair-helper();
                                            }
                                        }
                                    }
                                }
                            }
                            Text { text: "Streaming uses bundled yt-dlp + ffmpeg â€” nothing to install or update by hand."; color: Palette.text-faint; font-size: 10px; wrap: word-wrap; }
                            Text { text: "If a track won't open or playback breaks, click Repair â€” SideTone downloads the latest yt-dlp from the official yt-dlp GitHub project and restarts."; color: Palette.text-faint; font-size: 10px; wrap: word-wrap; }
                            Text { text: "Local downloads always play, even when streaming is rate-limited or blocked."; color: Palette.text-faint; font-size: 10px; wrap: word-wrap; }
                            Rectangle { vertical-stretch: 1; background: transparent; }
                        }

                        // â”€â”€ TAB 1 â€” Shortcuts (global hotkeys) â”€â”€
                        if root.settings-tab == 1: keycap := FocusScope {
                            key-pressed(event) => {
                                if (root.hotkey-capturing >= 0) {
                                    if (event.text == Key.Escape) {
                                        root.hotkey-capturing = -1;
                                        root.cap-armed = false;
                                    } else {
                                        root.cap-ctrl = event.modifiers.control;
                                        root.cap-alt = event.modifiers.alt;
                                        root.cap-shift = event.modifiers.shift;
                                        root.cap-meta = event.modifiers.meta;
                                        if (event.text != Key.Shift
                                                && event.text != Key.Control
                                                && event.text != Key.Alt
                                                && event.text != Key.Meta) {
                                            root.cap-key = event.text;
                                            root.cap-armed = true;
                                        }
                                    }
                                    accept
                                } else {
                                    reject
                                }
                            }
                            key-released(event) => {
                                if (root.hotkey-capturing >= 0) {
                                    if (root.cap-armed) {
                                        root.cap-armed = false;
                                        root.hotkey-captured(
                                            root.hotkey-capturing,
                                            root.cap-ctrl,
                                            root.cap-alt,
                                            root.cap-shift,
                                            root.cap-meta,
                                            root.cap-key);
                                    }
                                    accept
                                } else {
                                    reject
                                }
                            }
                            VerticalLayout {
                                x: 0; y: 0; width: parent.width; height: parent.height;
                                alignment: start;
                                spacing: 6px;
                                Rectangle {
                                    height: 16px;
                                    background: transparent;
                                    Text {
                                        x: 0px; width: parent.width - 56px; height: parent.height;
                                        text: "Shortcuts";
                                        color: Palette.text; font-size: 13px; font-weight: 700;
                                        vertical-alignment: center;
                                    }
                                    Text {
                                        x: parent.width - 52px; width: 52px; height: parent.height;
                                        text: "Reset";
                                        color: reset-hotkeys-ta.has-hover ? Palette.clay-bright : Palette.moss;
                                        font-size: 11px; font-weight: 600;
                                        horizontal-alignment: right; vertical-alignment: center;
                                    }
                                    reset-hotkeys-ta := TouchArea {
                                        x: parent.width - 56px; width: 56px; height: parent.height;
                                        clicked => { root.hotkey-reset(); }
                                    }
                                }
                                for cell[i] in root.hotkey-items: HotkeyCell {
                                    height: 30px;
                                    label: cell.label;
                                    combo: cell.combo;
                                    index: i;
                                    capturing: root.hotkey-capturing == i;
                                    activate(idx) => { root.cap-armed = false; root.hotkey-capturing = idx; keycap.focus(); }
                                }
                                Rectangle { height: 2px; background: transparent; }
                                Text {
                                    text: "Click a combo, then press the new keys. Reset restores defaults.";
                                    color: Palette.text-faint;
                                    font-size: 10px; wrap: word-wrap;
                                }
                                if root.hotkey-status != "": Text {
                                    text: "âš  " + root.hotkey-status;
                                    color: Palette.clay-bright;
                                    font-size: 10px; font-weight: 600; wrap: word-wrap;
                                }
                            }
                        }

                        // â”€â”€ TAB 2 â€” Output (audio device list) â”€â”€
                        if root.settings-tab == 2: VerticalLayout {
                            spacing: 8px;
                            HorizontalLayout {
                                height: 16px;
                                Text {
                                    text: "Audio output";
                                    color: Palette.text; font-size: 13px; font-weight: 700;
                                    vertical-alignment: center;
                                }
                                Rectangle { horizontal-stretch: 1; background: transparent; }
                                Text {
                                    text: root.output-label;
                                    color: Palette.moss; font-size: 11px; font-weight: 600;
                                    vertical-alignment: center; overflow: elide;
                                }
                            }
                            Rectangle {
                                vertical-stretch: 1;
                                background: Palette.window.with-alpha(0.45);
                                border-radius: 9px;
                                border-width: 1px; border-color: Palette.hairline.with-alpha(0.6);
                                Flickable {
                                    x: 6px; y: 5px; width: parent.width - 12px; height: parent.height - 10px;
                                    viewport-width: self.width;
                                    viewport-height: root.output-items.length * 30px;
                                    VerticalLayout {
                                        width: parent.width; spacing: 2px;
                                        for row in root.output-items: Rectangle {
                                            height: 28px;
                                            background: row.active ? Palette.active-bg : (out-row-ta.has-hover ? Palette.surface-hi : transparent);
                                            border-radius: 7px;
                                            Rectangle {
                                                x: 5px; y: 8px; width: 2px; height: 12px; border-radius: 1px;
                                                background: row.active ? Palette.moss : transparent;
                                            }
                                            Text {
                                                x: 15px; width: parent.width - 20px; height: parent.height;
                                                text: row.title;
                                                color: row.active ? Palette.moss : Palette.text-dim;
                                                font-size: 11px; vertical-alignment: center; overflow: elide;
                                            }
                                            out-row-ta := TouchArea {
                                                clicked => { root.output-select(row.index); }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // â”€â”€ Delete confirmation dialog â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
        if root.confirm-open: Rectangle {
            width: 100%; height: 100%;
            background: Palette.dim.with-alpha(0.5);
            TouchArea { clicked => { root.confirm-open = false; } }
            Rectangle {
                width: 308px; height: 150px;
                x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: Palette.surface;
                border-radius: 14px;
                border-width: 1px; border-color: Palette.hairline;
                TouchArea {}
                VerticalLayout {
                    padding: 18px; spacing: 16px;
                    Text {
                        vertical-stretch: 1;
                        text: root.confirm-message;
                        color: Palette.text; font-size: 13px; font-weight: 600;
                        wrap: word-wrap; horizontal-alignment: center; vertical-alignment: center;
                    }
                    HorizontalLayout {
                        height: 36px; spacing: 10px;
                        Rectangle {
                            horizontal-stretch: 1; border-radius: 9px;
                            background: cancel-ta.has-hover ? Palette.surface-hi : Palette.window;
                            border-width: 1px; border-color: Palette.hairline;
                            Text { width: 100%; height: 100%; text: "Cancel"; color: Palette.text-dim; font-size: 12px; font-weight: 600; horizontal-alignment: center; vertical-alignment: center; }
                            cancel-ta := TouchArea { clicked => { root.confirm-open = false; } }
                        }
                        Rectangle {
                            horizontal-stretch: 1; border-radius: 9px;
                            background: del-ta.has-hover ? Palette.clay-bright : Palette.clay;
                            Text { width: 100%; height: 100%; text: "Delete"; color: Palette.sheen; font-size: 12px; font-weight: 700; horizontal-alignment: center; vertical-alignment: center; }
                            del-ta := TouchArea {
                                clicked => {
                                    if root.pending-action == "download" { root.queue-remove(root.pending-url); }
                                    if root.pending-action == "playlist" { root.playlist-delete(root.pending-url); }
                                    if root.pending-action == "selection" { root.delete-selection(); }
                                    root.confirm-open = false;
                                }
                            }
                        }
                    }
                }
            }
        }

        // â”€â”€ Add-to-playlist picker (create new or add to a saved one) â”€
        if root.picker-open: Rectangle {
            width: 100%; height: 100%;
            background: Palette.dim.with-alpha(0.5);
            TouchArea { clicked => { root.picker-open = false; } }
            Rectangle {
                width: 312px; height: 268px;
                x: (parent.width - self.width) / 2; y: (parent.height - self.height) / 2;
                background: Palette.surface;
                border-radius: 14px;
                border-width: 1px; border-color: Palette.hairline;
                TouchArea {}
                VerticalLayout {
                    padding: 16px; spacing: 10px;
                    Text {
                        text: "ADD " + root.selected-count + " TRACK" + (root.selected-count == 1 ? "" : "S") + " TO";
                        color: Palette.text-dim; font-size: 9px; font-weight: 700; letter-spacing: 1.5px;
                        height: 12px;
                    }
                    Rectangle {
                        height: 38px; border-radius: 9px;
                        background: new-ta.has-hover ? Palette.surface-hi : transparent;
                        border-width: 1px; border-color: Palette.hairline-hi;
                        Text { x: 12px; width: parent.width - 20px; height: parent.height; text: "+  Create new playlist"; color: Palette.clay; font-size: 12px; font-weight: 600; vertical-alignment: center; }
                        new-ta := TouchArea { clicked => { root.picker-open = false; root.create-playlist(); } }
                    }
                    Rectangle {
                        vertical-stretch: 1; background: transparent;
                        if root.playlist-items.length == 0: Text {
                            width: parent.width; height: parent.height;
                            text: "No saved playlists yet â€”\ncreate one above.";
                            color: Palette.text-faint; font-size: 11px;
                            horizontal-alignment: center; vertical-alignment: center; wrap: word-wrap;
                        }
                        Flickable {
                            width: parent.width; height: parent.height;
                            viewport-width: self.width;
                            viewport-height: root.playlist-items.length * 38px;
                            VerticalLayout {
                                width: parent.width; spacing: 3px;
                                for row in root.playlist-items: Rectangle {
                                    height: 35px; border-radius: 8px;
                                    background: pick-ta.has-hover ? Palette.surface-hi : transparent;
                                    Text { x: 12px; width: parent.width - 20px; height: parent.height; text: "â™ª  " + row.title; color: Palette.text; font-size: 12px; vertical-alignment: center; overflow: elide; }
                                    pick-ta := TouchArea { clicked => { root.add-to-playlist(row.url); root.picker-open = false; } }
                                }
                            }
                        }
                    }
                    Rectangle {
                        height: 32px; border-radius: 9px;
                        background: pick-cancel-ta.has-hover ? Palette.surface-hi : Palette.window;
                        border-width: 1px; border-color: Palette.hairline;
                        Text { width: 100%; height: 100%; text: "Cancel"; color: Palette.text-dim; font-size: 12px; font-weight: 600; horizontal-alignment: center; vertical-alignment: center; }
                        pick-cancel-ta := TouchArea { clicked => { root.picker-open = false; } }
                    }
                }
            }
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return run_app();
    };

    match command.as_str() {
        "scan" => {
            let Some(root) = args.next() else {
                eprintln!("Missing music folder path.");
                print_usage();
                std::process::exit(1);
            };

            scan_command(Path::new(&root)).map_err(|error| format!("Scan failed: {error}"))?;
        }
        "play" => {
            let Some(path) = args.next() else {
                eprintln!("Missing audio file or folder path.");
                print_usage();
                std::process::exit(1);
            };

            play_command(Path::new(&path)).map_err(|error| format!("Playback failed: {error}"))?;
        }
        "tone" => {
            tone_command().map_err(|error| format!("Tone failed: {error}"))?;
        }
        "yt-search" => {
            let query = args.collect::<Vec<_>>().join(" ");
            if query.is_empty() {
                eprintln!("Missing YouTube search query.");
                print_usage();
                std::process::exit(1);
            }

            yt_search_command(&query).map_err(|error| format!("YouTube search failed: {error}"))?;
        }
        "yt-play" | "yt" => {
            let input = args.collect::<Vec<_>>().join(" ");
            if input.is_empty() {
                eprintln!("Missing YouTube URL or search query.");
                print_usage();
                std::process::exit(1);
            }

            yt_play_command(&input).map_err(|error| format!("YouTube playback failed: {error}"))?;
        }
        "yt-resolve" => {
            let input = args.collect::<Vec<_>>().join(" ");
            if input.is_empty() {
                eprintln!("Missing YouTube URL or search query.");
                print_usage();
                std::process::exit(1);
            }

            yt_resolve_command(&input)
                .map_err(|error| format!("YouTube resolve failed: {error}"))?;
        }
        _ => {
            eprintln!("Unknown command: {command}");
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}

fn run_app() -> AppResult<()> {
    // v7: single-instance guard. If SideTone is already running, surface that
    // window (restore from tray / foreground when Windows allows) and exit this
    // second process cleanly instead of opening a duplicate window.
    #[cfg(windows)]
    if !acquire_single_instance() {
        return Ok(());
    }

    // v7: move user data to %LOCALAPPDATA%\SideTone\Data, copying any pre-v7
    // exe-adjacent data once. Safe + best-effort â€” must run before any data read.
    #[cfg(windows)]
    migrate_legacy_data_if_needed();

    // Best-effort: remove buffered-audio temp folders leaked by earlier runs
    // that exited without dropping their controller (crash or the post-update
    // relaunch). The single-instance guard above guarantees no other live
    // SideTone owns them.
    sweep_stale_temp_dirs();

    let app = AppWindow::new()?;
    let theme = load_theme_config();
    app.set_theme_mode(theme);
    app.global::<Palette>().set_theme(theme);
    app.on_theme_changed(save_theme_config);

    let output_devices = Arc::new(enumerate_output_devices());
    let controller = Arc::new(Mutex::new(PlayerController::new(
        output_devices
            .first()
            .and_then(|device| device.device.clone()),
    )?));
    if let Some(device) = output_devices.first() {
        app.set_output_label(SharedString::from(short_device_label(&device.name)));
    }
    app.set_output_items(output_model(&output_devices, 0));
    let queue = Arc::new(Mutex::new(AppQueue::default()));
    let import_cancel = Arc::new(AtomicBool::new(false));
    bind_app_callbacks(
        &app,
        Arc::clone(&controller),
        Arc::clone(&queue),
        Arc::clone(&output_devices),
        Arc::clone(&import_cancel),
    );
    bind_helper_status(&app, Arc::clone(&controller));
    bind_panel_window_resizing(&app);
    bind_layout_controls(&app);

    // Update check: banner click opens the hardcoded, validated releases page
    // (never a remote-supplied URL); X dismisses it.
    app.on_open_update(open_update_page);
    app.on_dismiss_update({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                app.set_update_available(false);
            }
        }
    });
    check_for_update(app.as_weak());

    let progress_timer = start_progress_timer(&app, Arc::clone(&controller), Arc::clone(&queue));

    // Apply the Windows acrylic glass backdrop once the native window exists.
    #[cfg(windows)]
    slint::Timer::single_shot(std::time::Duration::from_millis(80), apply_window_blur);

    // Phase 1: OS media controls (SMTC) + global hotkeys. Kept alive until run() returns.
    let _native = setup_native_presence(&app);

    app.run()?;
    progress_timer.stop();
    Ok(())
}

fn bind_panel_window_resizing(app: &AppWindow) {
    let previous_extra = Rc::new(Cell::new(current_panel_extra(app)));
    app.on_panel_layout_changed({
        let weak = app.as_weak();
        let previous_extra = Rc::clone(&previous_extra);
        move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            resize_window_for_panel_delta(&app, &previous_extra);
        }
    });
}

fn resize_window_for_panel_delta(app: &AppWindow, previous_extra: &Cell<f32>) {
    let next_extra = current_panel_extra(app);
    let delta = next_extra - previous_extra.get();
    previous_extra.set(next_extra);

    if delta.abs() < 0.5 {
        return;
    }

    let window = app.window();
    if window.is_maximized() || window.is_fullscreen() {
        return;
    }

    let scale = window.scale_factor();
    let current = window.size().to_logical(scale);
    let min_height = 640.0 + next_extra;
    let next_height = (current.height + delta).max(min_height);
    window.set_size(LogicalSize::new(current.width, next_height));
}

fn current_panel_extra(app: &AppWindow) -> f32 {
    // Settings now floats as an overlay (it does not grow the window). Only the
    // TUNE popover still expands the window height.
    if app.get_tune_open() {
        204.0
    } else {
        0.0
    }
}

#[cfg(windows)]
fn apply_window_blur() {
    use windows::core::w;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE,
    };
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    unsafe {
        let hwnd = match FindWindowW(None, w!("SideTone")) {
            Ok(hwnd) => hwnd,
            Err(_) => return,
        };
        // DWMSBT_TRANSIENTWINDOW (3) = acrylic â€” real see-through blur of the
        // windows behind. Dims slightly when the window is inactive (inherent
        // to acrylic); preferred over Mica which only tints the wallpaper.
        let backdrop: i32 = 3;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const i32 as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        );
        // DWMWCP_ROUND (2) = rounded window corners
        let corner: i32 = 2;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const i32 as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        );
        // v4 light theme: use a LIGHT title bar so it blends into the
        // parchment content instead of a stark dark caption bar.
        let dark: i32 = 0;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const i32 as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        );
    }
}

// ----------------------------------------------------------------------------
// Phase 1: native OS presence â€” global hotkeys + system media controls (SMTC).
// OS/external-thread events are marshaled onto the Slint event loop and then
// call the SAME generated `invoke_*` callbacks the in-app buttons use, so all
// playback logic stays in bind_app_callbacks (single source of truth).
// ----------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum MediaAction {
    PlayPause,
    Next,
    Prev,
    ToggleFocus,
    Stop,
}

fn run_media_action(app: &AppWindow, action: MediaAction) {
    // While the streaming engine is being repaired, external triggers (global
    // hotkeys, tray menu) must not start playback. Stop/Focus are harmless (they
    // never spawn a helper), so they stay live.
    if streaming_helpers::repair_in_progress()
        && matches!(
            action,
            MediaAction::PlayPause | MediaAction::Next | MediaAction::Prev
        )
    {
        app.set_status_text(SharedString::from(
            "Updating the streaming engine â€” playback is paused for a moment.",
        ));
        app.set_status_flash(true);
        return;
    }
    match action {
        MediaAction::PlayPause => app.invoke_pause_toggle(),
        MediaAction::Next => app.invoke_queue_next(),
        MediaAction::Prev => app.invoke_queue_prev(),
        // The in-app FOCUS button flips `focus-enabled` before invoking the
        // callback; external triggers must flip it too or Focus never turns on.
        MediaAction::ToggleFocus => {
            app.set_focus_enabled(!app.get_focus_enabled());
            app.invoke_focus_toggle();
        }
        MediaAction::Stop => app.invoke_stop_playback(),
    }
}

/// Commands issued from the system-tray icon (menu items + left-click).
#[cfg(windows)]
#[derive(Clone, Copy)]
enum TrayCommand {
    Action(MediaAction),
    ShowHide,
    CycleLayout,
    Quit,
}

#[cfg(windows)]
fn apply_tray_command(app: &AppWindow, command: TrayCommand) {
    match command {
        TrayCommand::Action(action) => run_media_action(app, action),
        TrayCommand::ShowHide => {
            let window = app.window();
            if window.is_visible() {
                let _ = window.hide();
            } else {
                let _ = window.show();
                window.set_minimized(false);
            }
        }
        TrayCommand::CycleLayout => app.invoke_cycle_layout(),
        TrayCommand::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

/// Load a tray icon: embedded resource first, then the bundled .ico next to the exe.
#[cfg(windows)]
fn tray_icon_image() -> Option<tray_icon::Icon> {
    if let Ok(icon) = tray_icon::Icon::from_resource(1, None) {
        return Some(icon);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join("sidetone.ico"),
                dir.join("assets").join("sidetone.ico"),
            ] {
                if let Ok(icon) = tray_icon::Icon::from_path(&candidate, None) {
                    return Some(icon);
                }
            }
        }
    }
    tray_icon::Icon::from_path("assets/sidetone.ico", None).ok()
}

/// Resolve the native window handle by title (same approach as apply_window_blur).
#[cfg(windows)]
fn native_window_handle() -> Option<*mut core::ffi::c_void> {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    unsafe {
        match FindWindowW(None, w!("SideTone")) {
            Ok(hwnd) if !hwnd.0.is_null() => Some(hwnd.0),
            _ => None,
        }
    }
}

/// Windows single-instance guard. Returns `true` if this process may run (it is
/// the first instance), or `false` if SideTone is already running â€” in which
/// case we try to surface the existing window and the caller should exit.
///
/// Uses a named mutex: the OS releases it automatically when the holding process
/// dies, so a crash can never permanently lock out relaunch. The mutex handle is
/// intentionally leaked (held for the whole process lifetime). If the mutex can't
/// be created at all we fail open and allow launch â€” never block a real start.
#[cfg(windows)]
fn acquire_single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        // Stable, app-specific name (the embedded GUID keeps it from colliding
        // with any other app's mutex).
        let handle = match CreateMutexW(
            None,
            true,
            w!("SideTone-SingleInstance-{E7C2A14F-3B8D-4F91-B2A6-9D5C1E4F7A83}"),
        ) {
            Ok(handle) => handle,
            // Couldn't create the guard â€” fail open so launch is never blocked.
            Err(_) => return true,
        };

        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Another instance owns the mutex. Close our duplicate handle, try to
            // surface the existing window, and tell the caller to exit.
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            surface_existing_window();
            return false;
        }

        // First instance: keep the mutex held for the process lifetime. `HANDLE`
        // is `Copy` with no `Drop`, so simply not calling `CloseHandle` leaves it
        // open until the process exits (the OS then releases the named mutex).
        let _ = handle;
        true
    }
}

/// Best-effort: restore the existing SideTone window (it may be hidden to the
/// tray or minimized) and bring it to the foreground. Windows foreground rules
/// can block the raise; that's acceptable â€” the goal is "no duplicate, restore
/// when allowed," not forcing focus 100% of the time.
#[cfg(windows)]
fn surface_existing_window() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};

    if let Some(raw) = native_window_handle() {
        let hwnd = HWND(raw);
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

// --- Layout mode (Phase 2b) -------------------------------------------------
// 0 = Standard, 1 = Mini. Persisted to layout.json next to the exe,
// same pattern as theme.json. The view tree + window sizing land in later steps;
// Step 1 only plumbs the state so the choice survives restarts and is reachable
// from the tray.

const LAYOUT_LABELS: [&str; 2] = ["Standard", "Mini"];

/// Apply a layout mode: clamp, set the property, persist, resize, confirm.
fn apply_layout_mode(app: &AppWindow, mode: i32) {
    let mode = mode.clamp(0, 1);
    app.set_layout_mode(mode);
    save_layout_config(mode);
    resize_window_for_layout(app, mode);
    app.set_status_text(SharedString::from(format!(
        "Layout: {}",
        LAYOUT_LABELS[mode as usize]
    )));
}

/// Resize the window for a layout mode. Mini (1) is a fixed thin strip; Standard
/// (0) restores the full height plus any open popover (TUNE/output/keys) so it
/// isn't clipped. Width is preserved; the Slint min/preferred heights match.
fn resize_window_for_layout(app: &AppWindow, mode: i32) {
    let window = app.window();
    if window.is_maximized() || window.is_fullscreen() {
        return;
    }
    let scale = window.scale_factor();
    let current = window.size().to_logical(scale);
    let height = if mode == 1 {
        108.0
    } else {
        808.0 + current_panel_extra(app)
    };
    window.set_size(LogicalSize::new(current.width.max(452.0), height));
}

fn bind_layout_controls(app: &AppWindow) {
    app.set_layout_mode(load_layout_config());
    app.on_cycle_layout({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                let next = (app.get_layout_mode() + 1).rem_euclid(2);
                apply_layout_mode(&app, next);
            }
        }
    });
    app.on_set_layout({
        let weak = app.as_weak();
        move |mode| {
            if let Some(app) = weak.upgrade() {
                apply_layout_mode(&app, mode);
            }
        }
    });
}

fn bind_helper_status(app: &AppWindow, controller: Arc<Mutex<PlayerController>>) {
    refresh_helper_status(app.as_weak());
    app.on_refresh_helper_status({
        let weak = app.as_weak();
        move || refresh_helper_status(weak.clone())
    });
    app.on_repair_helper({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move || repair_streaming_helpers(weak.clone(), Arc::clone(&controller))
    });
}

fn repair_streaming_helpers(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
) {
    // Never start a second repair while one is in flight. This same flag also
    // blocks every yt-dlp/ffmpeg spawn for the duration (see streaming_helpers),
    // so nothing can re-lock yt-dlp.exe while we swap it.
    if !streaming_helpers::try_begin_repair() {
        return;
    }

    if let Some(app) = weak.upgrade() {
        app.set_streaming_helper_label(SharedString::from("Repairing streaming helpers..."));
        app.set_streaming_helper_action(SharedString::from("Repairing"));
        app.set_status_text(SharedString::from("Repairing streaming helpers..."));
        app.set_status_flash(true);
    }

    // Item 7: stop playback first so no live yt-dlp/ffmpeg child holds a lock on
    // yt-dlp.exe while we try to replace it. Dropping the player tears down the
    // stream pipeline (kills + waits its children).
    if let Ok(mut controller) = controller.lock() {
        controller.stop();
    }
    if let Some(app) = weak.upgrade() {
        app.set_playback_active(false);
        app.set_playback_paused(false);
    }

    let restart_controller = Arc::clone(&controller);
    thread::spawn(move || {
        let result = repair_streaming_helpers_blocking().map_err(|error| error.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            streaming_helpers::end_repair();
            if let Some(app) = weak.upgrade() {
                match result {
                    Ok(label) => {
                        app.set_streaming_helper_label(SharedString::from(label));
                        app.set_streaming_helper_action(SharedString::from("Check"));
                        app.set_status_text(SharedString::from(
                            "Streaming engine updated. Restarting SideTone...",
                        ));
                        app.set_status_flash(true);
                        // Relaunch so playback picks up the fresh yt-dlp cleanly.
                        let restart_controller = Arc::clone(&restart_controller);
                        slint::Timer::single_shot(Duration::from_millis(1200), move || {
                            restart_app(restart_controller)
                        });
                        return;
                    }
                    Err(error) => {
                        // Map the raw error to a clear, specific failure category
                        // (missing / blocked / network / verification / self-test
                        // / ffmpeg / disallowed source). The old helper is always
                        // left intact on failure.
                        let kind = streaming_helpers::classify_helper_failure(&error);
                        app.set_streaming_helper_label(SharedString::from(kind.message()));
                        app.set_streaming_helper_action(SharedString::from("Repair"));
                        app.set_status_text(SharedString::from(format!(
                            "Repair failed: {}",
                            kind.message()
                        )));
                    }
                }
                app.set_status_flash(true);
            }
        });
    });
}

/// Relaunch SideTone from its own executable, then exit the current process.
/// Used after a streaming-engine update so the new yt-dlp is picked up cleanly.
/// Stops the controller first: `process::exit` skips destructors, so without
/// this the active stream's `yt-dlp`/`ffmpeg` children would be orphaned (their
/// cleanup lives in `YoutubeStreamSource::drop`, run via `controller.stop()`).
fn restart_app(controller: Arc<Mutex<PlayerController>>) {
    // Recover from a poisoned mutex (a thread panicked while holding the lock)
    // so cleanup still runs — otherwise process::exit would orphan the children.
    let mut controller = controller
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    controller.stop();
    drop(controller);
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
    std::process::exit(0);
}

fn refresh_helper_status(weak: slint::Weak<AppWindow>) {
    if let Some(app) = weak.upgrade() {
        app.set_streaming_helper_label(SharedString::from("Checking bundled helpers..."));
        app.set_streaming_helper_action(SharedString::from("Check"));
    }
    thread::spawn(move || {
        let status = streaming_helper_status();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                app.set_streaming_helper_label(SharedString::from(status.label));
                app.set_streaming_helper_action(SharedString::from(status.action));
            }
        });
    });
}

// --- Configurable global hotkeys -------------------------------------------

const HOTKEY_LABELS: [&str; 4] = [
    "Play / Pause",
    "Toggle Focus",
    "Previous track",
    "Next track",
];

/// Map a hotkey slot index to the playback action it triggers.
fn hotkey_action(index: usize) -> MediaAction {
    match index {
        0 => MediaAction::PlayPause,
        1 => MediaAction::ToggleFocus,
        2 => MediaAction::Prev,
        _ => MediaAction::Next,
    }
}

/// User-customizable combos, persisted next to the executable as hotkeys.json.
/// Combos are stored canonically (e.g. "Ctrl+Alt+KeyP") and prettified for the UI.
#[derive(Clone, Serialize, Deserialize)]
struct HotkeyConfig {
    play_pause: String,
    focus: String,
    prev: String,
    next: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            play_pause: "Ctrl+Alt+KeyP".to_string(),
            focus: "Ctrl+Alt+KeyF".to_string(),
            prev: "Ctrl+Alt+Comma".to_string(),
            next: "Ctrl+Alt+Period".to_string(),
        }
    }
}

impl HotkeyConfig {
    fn get(&self, index: usize) -> &str {
        match index {
            0 => &self.play_pause,
            1 => &self.focus,
            2 => &self.prev,
            _ => &self.next,
        }
    }
    fn set(&mut self, index: usize, value: String) {
        match index {
            0 => self.play_pause = value,
            1 => self.focus = value,
            2 => self.prev = value,
            _ => self.next = value,
        }
    }
}

fn hotkeys_config_path() -> Option<PathBuf> {
    Some(data_dir()?.join("hotkeys.json"))
}

fn load_hotkey_config() -> HotkeyConfig {
    hotkeys_config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<HotkeyConfig>(&text).ok())
        .unwrap_or_default()
}

fn save_hotkey_config(config: &HotkeyConfig) {
    if let Some(path) = hotkeys_config_path() {
        let _ = write_json_atomic(&path, config);
    }
}

/// Parse a stored combo ("Ctrl+Alt+KeyP") into modifiers + key code.
fn parse_combo(
    combo: &str,
) -> Option<(
    global_hotkey::hotkey::Modifiers,
    global_hotkey::hotkey::Code,
)> {
    use global_hotkey::hotkey::{Code, Modifiers};
    use std::str::FromStr;
    let mut mods = Modifiers::empty();
    let mut code: Option<Code> = None;
    for raw in combo.split('+') {
        match raw.trim() {
            "" => {}
            "Ctrl" | "Control" => mods |= Modifiers::CONTROL,
            "Alt" | "Option" => mods |= Modifiers::ALT,
            "Shift" => mods |= Modifiers::SHIFT,
            "Meta" | "Win" | "Super" | "Cmd" => mods |= Modifiers::META,
            other => code = Code::from_str(other).ok(),
        }
    }
    code.map(|c| (mods, c))
}

/// Turn a captured keypress into a canonical combo string. Enforces a 2â€“3 key
/// combo: at least one real modifier (Ctrl/Alt/Win) so it can't fire during
/// normal typing, and at most two modifiers total. Shift counts toward the max
/// but can't stand alone (Shift+letter would clash with capitalization).
fn combo_from_capture(
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    text: &str,
) -> Result<String, &'static str> {
    let code_name = code_name_from_text(text).ok_or("Use a letter, number, or symbol")?;
    if !(ctrl || alt || meta) {
        return Err("Add Ctrl, Alt, or Win to the combo");
    }
    let modifier_count = [ctrl, alt, shift, meta].iter().filter(|m| **m).count();
    if modifier_count > 2 {
        return Err("Use at most 3 keys (two modifiers)");
    }
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if meta {
        parts.push("Win");
    }
    parts.push(&code_name);
    Ok(parts.join("+"))
}

/// Map the text from a Slint KeyEvent to a W3C key-code name (e.g. "KeyP").
/// Rejects control/special keys (Slint encodes modifiers and Esc/Tab/arrows as
/// chars below 0x20, or as multi-char names) â€” modifiers come via the bool args.
fn code_name_from_text(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None; // multi-char special key (arrows, F-keys)
    }
    if (c as u32) < 0x20 {
        return None; // control code: Shift/Ctrl/Alt/Meta/Esc/Tab/Return/â€¦
    }
    let letter = c.to_ascii_lowercase();
    if letter.is_ascii_alphabetic() {
        Some(format!("Key{}", letter.to_ascii_uppercase()))
    } else if letter.is_ascii_digit() {
        Some(format!("Digit{letter}"))
    } else {
        let name = match letter {
            ',' => "Comma",
            '.' => "Period",
            '/' => "Slash",
            ';' => "Semicolon",
            '\'' => "Quote",
            '[' => "BracketLeft",
            ']' => "BracketRight",
            '\\' => "Backslash",
            '-' => "Minus",
            '=' => "Equal",
            '`' => "Backquote",
            ' ' => "Space",
            _ => return None,
        };
        Some(name.to_string())
    }
}

/// Prettify a stored combo for display: "Ctrl+Alt+KeyP" -> "Ctrl + Alt + P".
fn pretty_combo(combo: &str) -> String {
    combo
        .split('+')
        .map(|raw| {
            let part = raw.trim();
            if let Some(rest) = part.strip_prefix("Key") {
                rest.to_string()
            } else if let Some(rest) = part.strip_prefix("Digit") {
                rest.to_string()
            } else {
                match part {
                    "Comma" => ",",
                    "Period" => ".",
                    "Slash" => "/",
                    "Semicolon" => ";",
                    "Quote" => "'",
                    "BracketLeft" => "[",
                    "BracketRight" => "]",
                    "Backslash" => "\\",
                    "Minus" => "-",
                    "Equal" => "=",
                    "Backquote" => "`",
                    "Control" => "Ctrl",
                    other => other,
                }
                .to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Build the Settings "Shortcuts" status line. Empty string = all good (the UI
/// hides it). Pure, so it's unit-testable without the OS hotkey manager.
/// `slots` is `(label, combo_parsed, registered)` per hotkey row.
fn hotkey_status_label(manager_ok: bool, slots: &[(&str, bool, bool)]) -> String {
    if !manager_ok {
        return "Global hotkeys are unavailable on this system.".to_string();
    }
    let failed: Vec<&str> = slots
        .iter()
        .filter(|(_, parsed, registered)| *parsed && !*registered)
        .map(|(label, _, _)| *label)
        .collect();
    if failed.is_empty() {
        String::new()
    } else {
        format!(
            "Couldn't register: {} â€” may already be in use by another app.",
            failed.join(", ")
        )
    }
}

fn hotkey_row_model(config: &HotkeyConfig) -> ModelRc<HotkeyRow> {
    let rows: Vec<HotkeyRow> = (0..HOTKEY_LABELS.len())
        .map(|i| HotkeyRow {
            label: SharedString::from(HOTKEY_LABELS[i]),
            combo: SharedString::from(pretty_combo(config.get(i))),
            capturing: false,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Owns the hotkey manager and the current registrations so combos can be
/// rebound at runtime without restarting the app.
struct HotkeyRuntime {
    manager: global_hotkey::GlobalHotKeyManager,
    config: HotkeyConfig,
    registered: [Option<global_hotkey::hotkey::HotKey>; 4],
    id_to_action: std::collections::HashMap<u32, usize>,
}

impl HotkeyRuntime {
    /// (Re)register the combo currently in `config` for one slot.
    fn register_index(&mut self, index: usize) {
        use global_hotkey::hotkey::HotKey;
        if let Some(old) = self.registered[index].take() {
            let _ = self.manager.unregister(old);
            self.id_to_action.remove(&old.id());
        }
        if let Some((mods, code)) = parse_combo(self.config.get(index)) {
            let hk = HotKey::new(Some(mods), code);
            if self.manager.register(hk).is_ok() {
                self.registered[index] = Some(hk);
                self.id_to_action.insert(hk.id(), index);
            }
        }
    }

    /// Current registration status for the Settings panel (empty = all good).
    fn status_label(&self) -> String {
        let slots: Vec<(&str, bool, bool)> = (0..HOTKEY_LABELS.len())
            .map(|index| {
                (
                    HOTKEY_LABELS[index],
                    parse_combo(self.config.get(index)).is_some(),
                    self.registered[index].is_some(),
                )
            })
            .collect();
        hotkey_status_label(true, &slots)
    }

    /// Try to bind a new combo to a slot. On failure, restore the previous one.
    fn rebind(&mut self, index: usize, combo: String) -> bool {
        use global_hotkey::hotkey::HotKey;
        let previous = self.config.get(index).to_string();
        if let Some(old) = self.registered[index].take() {
            let _ = self.manager.unregister(old);
            self.id_to_action.remove(&old.id());
        }
        self.config.set(index, combo);
        if let Some((mods, code)) = parse_combo(self.config.get(index)) {
            let hk = HotKey::new(Some(mods), code);
            if self.manager.register(hk).is_ok() {
                self.registered[index] = Some(hk);
                self.id_to_action.insert(hk.id(), index);
                save_hotkey_config(&self.config);
                return true;
            }
        }
        // Revert to the previous combo.
        self.config.set(index, previous);
        self.register_index(index);
        false
    }
}

/// Keeps OS-integration handles (hotkey manager, media controls, polling timers)
/// alive for the lifetime of the app window. Dropped after `app.run()` returns.
struct NativePresence {
    _hotkeys: Option<std::rc::Rc<std::cell::RefCell<HotkeyRuntime>>>,
    #[cfg(windows)]
    _media: std::rc::Rc<std::cell::RefCell<Option<souvlaki::MediaControls>>>,
    #[cfg(windows)]
    _tray: Option<tray_icon::TrayIcon>,
    _timers: Vec<Timer>,
}

fn setup_native_presence(app: &AppWindow) -> NativePresence {
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    let mut timers: Vec<Timer> = Vec::new();

    // ---- Global hotkeys: user-customizable combos, persisted in hotkeys.json.
    // The runtime owns the manager + current registrations so a rebind can
    // unregister the old combo and register the new one live.
    let hotkeys: Option<std::rc::Rc<std::cell::RefCell<HotkeyRuntime>>> =
        match GlobalHotKeyManager::new() {
            Ok(manager) => {
                let mut runtime = HotkeyRuntime {
                    manager,
                    config: load_hotkey_config(),
                    registered: [None, None, None, None],
                    id_to_action: std::collections::HashMap::new(),
                };
                for index in 0..HOTKEY_LABELS.len() {
                    runtime.register_index(index);
                }
                Some(std::rc::Rc::new(std::cell::RefCell::new(runtime)))
            }
            Err(_) => None,
        };

    // Populate the settings list with the current combos, and surface whether
    // hotkeys are actually available / registered.
    {
        let config = hotkeys
            .as_ref()
            .map(|rt| rt.borrow().config.clone())
            .unwrap_or_default();
        app.set_hotkey_items(hotkey_row_model(&config));
        let status = match hotkeys.as_ref() {
            Some(rt) => rt.borrow().status_label(),
            None => hotkey_status_label(false, &[]),
        };
        app.set_hotkey_status(SharedString::from(status));
    }

    // Poll for hotkey presses on the UI thread and route to the matching action.
    if let Some(runtime) = hotkeys.clone() {
        let weak = app.as_weak();
        let poll = Timer::default();
        poll.start(TimerMode::Repeated, Duration::from_millis(120), move || {
            while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state != HotKeyState::Pressed {
                    continue;
                }
                let action = runtime.borrow().id_to_action.get(&event.id).copied();
                if let (Some(index), Some(app)) = (action, weak.upgrade()) {
                    run_media_action(&app, hotkey_action(index));
                }
            }
        });
        timers.push(poll);
    }

    // Rebind: a captured keypress replaces the combo for one action (live).
    {
        let weak = app.as_weak();
        let runtime = hotkeys.clone();
        app.on_hotkey_captured(move |index, ctrl, alt, shift, meta, text| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if index < 0 || index as usize >= HOTKEY_LABELS.len() {
                return;
            }
            let index = index as usize;
            let Some(runtime) = runtime.as_ref() else {
                return;
            };
            match combo_from_capture(ctrl, alt, shift, meta, text.as_str()) {
                Ok(combo) => {
                    let ok = runtime.borrow_mut().rebind(index, combo);
                    let config = runtime.borrow().config.clone();
                    app.set_hotkey_items(hotkey_row_model(&config));
                    app.set_hotkey_status(SharedString::from(runtime.borrow().status_label()));
                    app.set_hotkey_capturing(-1);
                    app.set_status_text(SharedString::from(if ok {
                        "Hotkey updated"
                    } else {
                        "That combo is unavailable"
                    }));
                }
                Err(message) => {
                    // Invalid combo â€” keep waiting and tell the user why.
                    app.set_status_text(SharedString::from(message));
                }
            }
        });
    }

    // Reset all hotkeys to their defaults.
    {
        let weak = app.as_weak();
        let runtime = hotkeys.clone();
        app.on_hotkey_reset(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if let Some(runtime) = runtime.as_ref() {
                {
                    let mut rt = runtime.borrow_mut();
                    rt.config = HotkeyConfig::default();
                    for index in 0..HOTKEY_LABELS.len() {
                        rt.register_index(index);
                    }
                    save_hotkey_config(&rt.config);
                }
                let config = runtime.borrow().config.clone();
                app.set_hotkey_items(hotkey_row_model(&config));
                app.set_hotkey_status(SharedString::from(runtime.borrow().status_label()));
            }
            app.set_hotkey_capturing(-1);
            app.set_status_text(SharedString::from("Hotkeys reset to defaults"));
        });
    }

    // ---- System media controls (SMTC: now-playing card + keyboard media keys).
    #[cfg(windows)]
    let media: std::rc::Rc<std::cell::RefCell<Option<souvlaki::MediaControls>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    // Create SMTC once the native HWND exists (just after the window is shown).
    #[cfg(windows)]
    {
        let media = std::rc::Rc::clone(&media);
        let weak = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(160), move || {
            let Some(hwnd) = native_window_handle() else {
                return;
            };
            let config = souvlaki::PlatformConfig {
                display_name: "SideTone",
                dbus_name: "sidetone",
                hwnd: Some(hwnd),
            };
            let Ok(mut controls) = souvlaki::MediaControls::new(config) else {
                return;
            };
            let handler_weak = weak.clone();
            let _ = controls.attach(move |event| {
                use souvlaki::MediaControlEvent as E;
                let action = match event {
                    E::Play | E::Pause | E::Toggle => Some(MediaAction::PlayPause),
                    E::Next => Some(MediaAction::Next),
                    E::Previous => Some(MediaAction::Prev),
                    E::Stop => Some(MediaAction::Stop),
                    _ => None,
                };
                if let Some(action) = action {
                    let w = handler_weak.clone();
                    let _ = w.upgrade_in_event_loop(move |app| run_media_action(&app, action));
                }
            });
            let _ = controls.set_playback(souvlaki::MediaPlayback::Paused { progress: None });
            *media.borrow_mut() = Some(controls);
        });
    }

    // Keep SMTC metadata/status in sync with the UI â€” cheap: pushes only on change.
    #[cfg(windows)]
    {
        let media = std::rc::Rc::clone(&media);
        let weak = app.as_weak();
        let mut last: Option<(String, bool)> = None;
        let sync = Timer::default();
        sync.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let title = app.get_now_title().to_string();
            let paused = app.get_playback_paused();
            let snapshot = (title.clone(), paused);
            if last.as_ref() == Some(&snapshot) {
                return;
            }
            last = Some(snapshot);
            if let Some(controls) = media.borrow_mut().as_mut() {
                let _ = controls.set_metadata(souvlaki::MediaMetadata {
                    title: Some(&title),
                    artist: Some("SideTone"),
                    ..Default::default()
                });
                let playback = if title == "Nothing playing" {
                    souvlaki::MediaPlayback::Stopped
                } else if paused {
                    souvlaki::MediaPlayback::Paused { progress: None }
                } else {
                    souvlaki::MediaPlayback::Playing { progress: None }
                };
                let _ = controls.set_playback(playback);
            }
        });
        timers.push(sync);
    }

    // ---- System tray icon: mini control menu + left-click show/hide.
    #[cfg(windows)]
    let tray = {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let menu = Menu::new();
        let show = MenuItem::new("Show / Hide", true, None);
        let play = MenuItem::new("Play / Pause", true, None);
        let prev = MenuItem::new("Previous", true, None);
        let next = MenuItem::new("Next", true, None);
        let focus = MenuItem::new("Toggle Focus", true, None);
        let layout = MenuItem::new("Cycle Layout", true, None);
        let quit = MenuItem::new("Quit SideTone", true, None);
        let _ = menu.append_items(&[
            &show,
            &PredefinedMenuItem::separator(),
            &play,
            &prev,
            &next,
            &PredefinedMenuItem::separator(),
            &focus,
            &layout,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);

        let commands: Vec<(tray_icon::menu::MenuId, TrayCommand)> = vec![
            (show.id().clone(), TrayCommand::ShowHide),
            (
                play.id().clone(),
                TrayCommand::Action(MediaAction::PlayPause),
            ),
            (prev.id().clone(), TrayCommand::Action(MediaAction::Prev)),
            (next.id().clone(), TrayCommand::Action(MediaAction::Next)),
            (
                focus.id().clone(),
                TrayCommand::Action(MediaAction::ToggleFocus),
            ),
            (layout.id().clone(), TrayCommand::CycleLayout),
            (quit.id().clone(), TrayCommand::Quit),
        ];

        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("SideTone");
        if let Some(icon) = tray_icon_image() {
            builder = builder.with_icon(icon);
        }

        match builder.build() {
            Ok(tray) => {
                let weak = app.as_weak();
                let poll = Timer::default();
                poll.start(TimerMode::Repeated, Duration::from_millis(120), move || {
                    while let Ok(event) = MenuEvent::receiver().try_recv() {
                        if let Some((_, command)) = commands.iter().find(|(id, _)| id == &event.id)
                        {
                            if let Some(app) = weak.upgrade() {
                                apply_tray_command(&app, *command);
                            }
                        }
                    }
                    let mut show_hide = false;
                    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            show_hide = true;
                        }
                    }
                    if show_hide {
                        if let Some(app) = weak.upgrade() {
                            apply_tray_command(&app, TrayCommand::ShowHide);
                        }
                    }
                });
                timers.push(poll);
                Some(tray)
            }
            Err(_) => None,
        }
    };

    NativePresence {
        _hotkeys: hotkeys,
        #[cfg(windows)]
        _media: media,
        #[cfg(windows)]
        _tray: tray,
        _timers: timers,
    }
}

#[derive(Clone)]
struct OutputDeviceInfo {
    name: String,
    device: Option<cpal::Device>,
}

fn enumerate_output_devices() -> Vec<OutputDeviceInfo> {
    let host = cpal::default_host();
    let default = host.default_output_device();
    let default_name = default.as_ref().map(device_label).unwrap_or_default();

    let mut devices = vec![OutputDeviceInfo {
        name: if default_name.is_empty() {
            "Default".to_string()
        } else {
            format!("Default ({default_name})")
        },
        device: default,
    }];

    if let Ok(outputs) = host.output_devices() {
        for device in outputs {
            let name = device_label(&device);
            if devices.iter().any(|known| known.name == name) {
                continue;
            }
            devices.push(OutputDeviceInfo {
                name,
                device: Some(device),
            });
        }
    }

    devices
}

fn device_label(device: &cpal::Device) -> String {
    device
        .description()
        .map(|description| description.name().to_string())
        .unwrap_or_else(|_| "Output".to_string())
}

fn open_output_sink(device: Option<cpal::Device>) -> AppResult<rodio::MixerDeviceSink> {
    match device {
        Some(device) => Ok(rodio::DeviceSinkBuilder::from_device(device)?.open_stream()?),
        None => Ok(rodio::DeviceSinkBuilder::open_default_sink()?),
    }
}

fn short_device_label(name: &str) -> String {
    let normalized = name
        .replace("Default", "")
        .replace(['(', ')'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let value = if normalized.is_empty() {
        "Default"
    } else {
        normalized.as_str()
    };
    truncate(value, 12)
}

fn start_progress_timer(
    app: &AppWindow,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
) -> Timer {
    let timer = Timer::default();
    let weak = app.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        // Park-on-blur (Phase 2b): when the window is hidden to the tray, keep
        // driving auto-advance / loop but skip the per-tick progress property
        // writes (and the re-render they'd trigger) â€” the bar isn't on screen.
        let parked = !app.window().is_visible();
        // NEVER block the UI thread here. A background thread can hold the
        // controller lock while it does slow I/O (e.g. the ffmpeg duration
        // probe when a local track starts). If we used a blocking lock(), the
        // message pump would stall and Windows would show "Not Responding".
        // try_lock means we simply skip this tick and update on the next one.
        let (snapshot, action) = match controller.try_lock() {
            Ok(mut ctrl) => {
                let action = ctrl.take_playback_action();
                let snapshot = if parked {
                    None
                } else {
                    Some(ctrl.progress_snapshot())
                };
                (snapshot, action)
            }
            Err(std::sync::TryLockError::WouldBlock) => return,
            Err(std::sync::TryLockError::Poisoned(_)) => (None, PlaybackAction::None),
        };

        if let Some(snapshot) = snapshot {
            app.set_progress_percent(snapshot.percent);
            app.set_progress_seekable(snapshot.seekable);
            let label = if snapshot.label.is_empty() {
                "0:00 / 0:00".to_string()
            } else {
                snapshot.label
            };
            app.set_progress_text(SharedString::from(label));
        }

        match action {
            PlaybackAction::None => {}
            PlaybackAction::LoopStream(track) => {
                stop_current_for_new_track(&weak, &controller, "Looping...");
                let gen = next_play_gen(&queue);
                play_queued_track(
                    weak.clone(),
                    Arc::clone(&controller),
                    Arc::clone(&queue),
                    track,
                    gen,
                    "Loop",
                );
            }
            PlaybackAction::LoopLocal(tracks) => {
                stop_current_for_new_track(&weak, &controller, "Looping...");
                let gen = next_play_gen(&queue);
                play_local_tracks(
                    weak.clone(),
                    Arc::clone(&controller),
                    Arc::clone(&queue),
                    tracks,
                    gen,
                    "Loop",
                );
            }
            PlaybackAction::Advance => {
                advance_to_next(weak.clone(), Arc::clone(&controller), Arc::clone(&queue));
            }
        }
    });
    timer
}

fn bind_app_callbacks(
    app: &AppWindow,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
    output_devices: Arc<Vec<OutputDeviceInfo>>,
    import_cancel: Arc<AtomicBool>,
) {
    app.on_youtube_submit({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        let import_cancel = Arc::clone(&import_cancel);
        move |input| {
            let value = input.trim().to_string();
            if value.is_empty() {
                cancel_search(&queue);
                set_status(&weak, "Ready");
                return;
            }

            // Don't start a second action while an import is in flight â€” it would
            // race the importer over the queue. The Stop button cancels first.
            let importing = weak
                .upgrade()
                .map(|a| a.get_import_active())
                .unwrap_or(false);
            if importing {
                set_status(&weak, "An import is running â€” press Stop first.");
                return;
            }

            if is_playlist_import_url(&value) {
                import_cancel.store(false, Ordering::Relaxed);
                if let Ok(mut queue) = queue.lock() {
                    queue.clear();
                }
                stop_current_for_new_track(&weak, &controller, "Reading playlistâ€¦");
                import_playlist_link(
                    weak.clone(),
                    Arc::clone(&controller),
                    Arc::clone(&queue),
                    Arc::clone(&import_cancel),
                    value,
                );
            } else if is_youtube_url(&value) {
                if let Ok(mut queue) = queue.lock() {
                    queue.clear();
                    queue.yt_items.clear();
                }
                stop_current_for_new_track(&weak, &controller, "Opening stream...");
                let gen = next_play_gen(&queue);
                let controller = Arc::clone(&controller);
                let queue_guard = Arc::clone(&queue);
                run_background(weak.clone(), "Streaming link.", move || {
                    let (title, duration, stream_target, stream) = prepare_youtube_stream(&value)?;
                    if play_gen_is_stale(&queue_guard, gen) {
                        return Ok(stale_update());
                    }
                    controller
                        .lock()
                        .map_err(|_| "player controller lock poisoned")?
                        .play_stream(stream, duration, stream_target, clean_title(&title))?;
                    Ok(AppUpdate {
                        now_title: Some(clean_title(&title)),
                        yt_results: Some(Vec::new()),
                        search_results: Some(Vec::new()),
                        now_playing_url: None,
                        status_text: Some("Playing.".to_string()),
                        queue_context: Some(QueueContext::Stream),
                    })
                });
            } else {
                start_youtube_search(weak.clone(), Arc::clone(&queue), value);
            }
        }
    });

    app.on_import_cancel({
        let weak = app.as_weak();
        let import_cancel = Arc::clone(&import_cancel);
        move || {
            import_cancel.store(true, Ordering::Relaxed);
            if let Some(app) = weak.upgrade() {
                app.set_import_active(false);
                app.set_status_text(SharedString::from("Import cancelled."));
            }
        }
    });

    app.on_youtube_refresh({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            let (slots, playing) = match queue.lock() {
                Ok(q) => (q.yt_items.clone(), q.now_playing_url.clone()),
                Err(_) => return,
            };
            if let Some(app) = weak.upgrade() {
                app.set_queue_items(queue_model(&slots, &playing));
                app.set_selected_count(0);
                app.set_search_results(search_model(&[]));
                app.set_playlist_dropdown_open(false);
                app.set_naming_playlist(false);
            }
        }
    });

    // Activate the Local tab. Show the last-opened local list if there is one;
    // otherwise land on the persisted Library index.
    app.on_local_activate({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            cancel_search(&queue);
            let current_kind = match queue.lock() {
                Ok(q) => q.local_kind.clone(),
                Err(_) => LocalKind::None,
            };
            let mut library_status = None;
            if matches!(current_kind, LocalKind::Library | LocalKind::None) {
                let (index, removed, save_error) = load_clean_library_index();
                let slots = index.tracks;
                library_status = save_error.or_else(|| {
                    (removed > 0).then(|| format!("Library cleaned up {removed} missing track(s)."))
                });
                if let Ok(mut q) = queue.lock() {
                    q.local_items = slots;
                    q.local_kind = LocalKind::Library;
                }
            }

            let (local_items, playing, kind) = match queue.lock() {
                Ok(q) => (
                    q.local_items.clone(),
                    q.now_playing_url.clone(),
                    q.local_kind.clone(),
                ),
                Err(_) => (Vec::new(), String::new(), LocalKind::None),
            };
            let (label, tab, show_playlists) = match kind {
                LocalKind::Playlist(name) => (name, 1, true),
                LocalKind::Downloads => ("DOWNLOADS".to_string(), 2, false),
                LocalKind::Library | LocalKind::None => ("LIBRARY".to_string(), 0, false),
            };
            let playlists = list_playlists().unwrap_or_default();
            if let Some(app) = weak.upgrade() {
                app.set_playlist_items(playlist_model(&playlists));
                app.set_local_list_label(SharedString::from(label));
                app.set_local_tab(tab);
                app.set_local_show_playlists(show_playlists);
                app.set_queue_items(queue_model(&local_items, &playing));
                app.set_selected_count(0);
                app.set_search_results(search_model(&[]));
                if let Some(status) = library_status {
                    app.set_status_text(SharedString::from(status));
                }
            }
        }
    });

    app.on_local_library({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            let (index, removed, save_error) = load_clean_library_index();
            let slots = index.tracks;
            let playing = match queue.lock() {
                Ok(mut q) => {
                    q.local_items = slots.clone();
                    q.local_kind = LocalKind::Library;
                    q.now_playing_url.clone()
                }
                Err(_) => String::new(),
            };
            if let Some(app) = weak.upgrade() {
                app.set_playlist_dropdown_open(false);
                app.set_local_tab(0);
                app.set_local_show_playlists(false);
                app.set_local_list_label(SharedString::from("LIBRARY"));
                app.set_queue_items(queue_model(&slots, &playing));
                app.set_selected_count(0);
                app.set_search_results(search_model(&[]));
                if let Some(status) = save_error.or_else(|| {
                    (removed > 0).then(|| format!("Library cleaned up {removed} missing track(s)."))
                }) {
                    app.set_status_text(SharedString::from(status));
                }
            }
        }
    });

    app.on_local_playlists({
        let weak = app.as_weak();
        move || {
            let playlists = list_playlists().unwrap_or_default();
            if let Some(app) = weak.upgrade() {
                app.set_playlist_dropdown_open(false);
                app.set_local_tab(1);
                app.set_local_show_playlists(true);
                app.set_local_list_label(SharedString::from("PLAYLISTS"));
                app.set_playlist_items(playlist_model(&playlists));
                app.set_queue_items(queue_model(&[], ""));
                app.set_selected_count(0);
                app.set_search_results(search_model(&[]));
            }
        }
    });

    app.on_youtube_add_to_queue({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |input| {
            add_track_to_queue(weak.clone(), Arc::clone(&queue), input.to_string());
        }
    });

    // Play a search result directly by URL â€” no AppQueue lookup needed.
    app.on_youtube_play_result({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        move |url| {
            let target = url.to_string();
            if target.trim().is_empty() {
                set_status(&weak, "Pick a result.");
                return;
            }
            // Immediately add to the YouTube list + highlight so the window expands.
            if let Some(app) = weak.upgrade() {
                // Grab title from current search results before clearing
                let search = app.get_search_results();
                let title_hint = (0..search.row_count())
                    .filter_map(|i| search.row_data(i))
                    .find(|row| row.url.as_str() == target)
                    .map(|row| row.title.to_string())
                    .unwrap_or_else(|| "Loading...".to_string());

                if let Ok(mut q) = queue.lock() {
                    if !q.yt_items.iter().any(|r| r.url == target) {
                        q.yt_items.push(YtResultSlot {
                            title: title_hint,
                            url: target.clone(),
                        });
                    }
                    let items = q.yt_items.clone();
                    q.results = items.clone();
                    q.current_index = items.iter().position(|r| r.url == target);
                    q.now_playing_url = target.clone();
                    q.playback_context = QueueContext::Stream;
                    app.set_queue_items(queue_model(&items, &target));
                    app.set_selected_count(0);
                }
                app.set_search_results(search_model(&[]));
            }
            stop_current_for_new_track(&weak, &controller, "Opening stream...");
            let gen = next_play_gen(&queue);
            let controller2 = Arc::clone(&controller);
            let queue2 = Arc::clone(&queue);
            run_background(weak.clone(), "Playing.", move || {
                let (title, duration, stream_target, stream) = prepare_youtube_stream(&target)?;
                if play_gen_is_stale(&queue2, gen) {
                    return Ok(stale_update());
                }
                let clean = clean_title(&title);
                controller2
                    .lock()
                    .map_err(|_| "controller lock poisoned")?
                    .play_stream(stream, duration, stream_target, clean.clone())?;

                // Update the YouTube list with the real title once the stream resolves.
                let display = {
                    let mut q = queue2.lock().map_err(|_| "queue lock poisoned")?;
                    if let Some(slot) = q.yt_items.iter_mut().find(|r| r.url == target) {
                        slot.title = clean.clone();
                    } else {
                        q.yt_items.push(YtResultSlot {
                            title: clean.clone(),
                            url: target.clone(),
                        });
                    }
                    let items = q.yt_items.clone();
                    q.results = items.clone();
                    q.current_index = items.iter().position(|r| r.url == target);
                    q.now_playing_url = target.clone();
                    q.playback_context = QueueContext::Stream;
                    items
                };

                Ok(AppUpdate {
                    now_title: Some(clean),
                    yt_results: Some(display),
                    search_results: Some(Vec::new()),
                    now_playing_url: Some(target.clone()),
                    status_text: Some("Playing.".to_string()),
                    queue_context: Some(QueueContext::Stream),
                })
            });
        }
    });

    // Add a search result to queue without playing.
    app.on_youtube_queue_result({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |url, title| {
            let slot = YtResultSlot {
                title: title.to_string(),
                url: url.to_string(),
            };
            let result = queue.lock().map(|mut q| {
                if !q.yt_items.iter().any(|r| r.url == slot.url) {
                    q.yt_items.push(slot);
                }
                (q.yt_items.clone(), q.now_playing_url.clone())
            });
            match result {
                Ok((all_slots, playing)) => {
                    if let Some(app) = weak.upgrade() {
                        app.set_queue_items(queue_model(&all_slots, &playing));
                        app.set_selected_count(0);
                        app.set_search_results(search_model(&[]));
                        app.set_status_text(SharedString::from(format!("Queued: {}", title)));
                    }
                }
                Err(_) => set_status(&weak, "Error: queue lock poisoned"),
            }
        }
    });

    app.on_local_filter({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |query| {
            if let Some(app) = weak.upgrade() {
                if app.get_source_mode() == "local" {
                    apply_local_filter(&app, &queue, &query);
                }
            }
        }
    });

    app.on_local_scan({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |input| {
            let raw = input.trim().to_string();
            if let Some(app) = weak.upgrade() {
                app.set_playlist_dropdown_open(false);
                app.set_local_tab(0);
                app.set_local_show_playlists(false);
                app.set_local_list_label(SharedString::from("LIBRARY"));
                // If the text isn't an existing path, Enter means "search" â€” run
                // the live filter instead of failing a folder scan.
                if !raw.is_empty() && !Path::new(&raw).exists() {
                    apply_local_filter(&app, &queue, &raw);
                    return;
                }
            }
            set_status(&weak, "Scanning music library...");
            let queue = Arc::clone(&queue);
            run_background(weak.clone(), "Scan complete.", move || {
                // No path typed â†’ scan the user's Music folder automatically.
                let root = if raw.is_empty() {
                    default_music_dir()
                        .ok_or("Could not find your Music folder. Paste a folder path instead.")?
                } else {
                    PathBuf::from(&raw)
                };
                if !root.exists() {
                    return Err(format!("Path not found: {}", root.display()).into());
                }
                let collections = scan_library(&root)?;
                let total: usize = collections.iter().map(|c| c.tracks.len()).sum();
                let slots = local_slots_for_collections(&collections);
                save_library_scan(&root, &slots)?;
                let playing = {
                    let mut q = queue.lock().map_err(|_| "queue lock poisoned")?;
                    q.local_items = slots.clone();
                    q.local_kind = LocalKind::Library;
                    q.now_playing_url.clone()
                };
                Ok(AppUpdate {
                    now_title: None,
                    yt_results: Some(slots),
                    search_results: None,
                    now_playing_url: Some(playing),
                    status_text: Some(if total == 0 {
                        "No audio files found.".to_string()
                    } else {
                        format!("Found {total} tracks.")
                    }),
                    queue_context: Some(QueueContext::Library),
                })
            });
        }
    });

    app.on_local_favorites({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            if let Some(app) = weak.upgrade() {
                app.set_playlist_dropdown_open(false);
                app.set_local_tab(2);
                app.set_local_show_playlists(false);
                app.set_local_list_label(SharedString::from("DOWNLOADS"));
            }
            set_status(&weak, "Loading downloads...");
            let queue = Arc::clone(&queue);
            run_background(weak.clone(), "Downloads loaded.", move || {
                let slots = favorite_slots()?;
                let count = slots.len();
                let playing = {
                    let mut q = queue.lock().map_err(|_| "queue lock poisoned")?;
                    q.local_items = slots.clone();
                    q.local_kind = LocalKind::Downloads;
                    q.now_playing_url.clone()
                };
                Ok(AppUpdate {
                    now_title: None,
                    yt_results: Some(slots),
                    search_results: None,
                    now_playing_url: Some(playing),
                    status_text: Some(if count == 0 {
                        "No downloads yet.".to_string()
                    } else {
                        "Downloads loaded.".to_string()
                    }),
                    queue_context: Some(QueueContext::Downloads),
                })
            });
        }
    });

    app.on_pause_toggle({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        move || {
            let playback_active = weak
                .upgrade()
                .map(|app| app.get_playback_active())
                .unwrap_or(false);
            if !playback_active {
                restart_queue_from_start(weak.clone(), Arc::clone(&controller), Arc::clone(&queue));
                return;
            }

            let result = controller
                .lock()
                .map_err(|_| "player controller lock poisoned")
                .map(|mut controller| controller.toggle_pause());

            match result {
                Ok(is_paused) => {
                    if let Some(app) = weak.upgrade() {
                        app.set_playback_paused(is_paused);
                        app.set_status_text(SharedString::from(if is_paused {
                            "Paused"
                        } else {
                            "Playing"
                        }));
                    }
                }
                Err(error) => set_status(&weak, &format!("Error: {error}")),
            }
        }
    });

    app.on_stop_playback({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move || {
            if let Ok(mut controller) = controller.lock() {
                controller.stop();
                if let Some(app) = weak.upgrade() {
                    app.set_playback_active(false);
                    app.set_playback_paused(false);
                    app.set_status_text(SharedString::from("Stopped"));
                }
            } else {
                set_status(&weak, "Error: player controller lock poisoned");
            }
        }
    });

    app.on_queue_prev({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        move || {
            play_queue_neighbor(
                weak.clone(),
                Arc::clone(&controller),
                Arc::clone(&queue),
                QueueDirection::Previous,
            );
        }
    });

    app.on_queue_next({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        move || {
            play_queue_neighbor(
                weak.clone(),
                Arc::clone(&controller),
                Arc::clone(&queue),
                QueueDirection::Next,
            );
        }
    });

    app.on_shuffle_queue({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let is_local = app.get_source_mode() == "local";
            if let Ok(mut q) = queue.lock() {
                let playing = q.now_playing_url.clone();
                let visible = if is_local {
                    q.local_items.clone()
                } else {
                    q.yt_items.clone()
                };
                let stored = if is_local {
                    q.local_shuffle.clone()
                } else {
                    q.yt_shuffle.clone()
                };
                // Active = we have a backup whose shuffled order still matches
                // what's on screen (i.e. the list wasn't rebuilt since).
                let active_now = stored.as_ref().is_some_and(|(_, sh)| *sh == visible);

                let (new_list, active) = if active_now {
                    // Deactivate: restore the saved original order.
                    let orig = stored.unwrap().0;
                    if is_local {
                        q.local_shuffle = None;
                    } else {
                        q.yt_shuffle = None;
                    }
                    (orig, false)
                } else {
                    // Activate: snapshot the current order, then shuffle.
                    if visible.len() < 2 {
                        return;
                    }
                    let mut shuffled = visible.clone();
                    shuffle_slots(&mut shuffled, &playing);
                    let store = Some((visible.clone(), shuffled.clone()));
                    if is_local {
                        q.local_shuffle = store;
                    } else {
                        q.yt_shuffle = store;
                    }
                    (shuffled, true)
                };

                if is_local {
                    q.local_items = new_list.clone();
                } else {
                    q.yt_items = new_list.clone();
                }
                // Reseed the playback queue so prev/next follows the shown order.
                q.current_index = new_list.iter().position(|r| r.url == playing);
                q.results = new_list.clone();
                app.set_queue_items(queue_model(&new_list, &playing));
                app.set_shuffle_active(active);
                app.set_status_text(SharedString::from(if active {
                    "Shuffled"
                } else {
                    "Original order"
                }));
            }
        }
    });

    app.on_seek_changed({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move |percent| {
            // Never block the UI thread. If a stream restart is already in
            // flight (lock held by the background resolver), drop this seek
            // event rather than stalling the message pump.
            let mut ctrl = match controller.try_lock() {
                Ok(ctrl) => ctrl,
                Err(_) => return,
            };

            // Local files: fast in-place seek, safe to do right here.
            if let Some(snapshot) = ctrl.seek_local(percent) {
                drop(ctrl);
                if let Some(app) = weak.upgrade() {
                    app.set_progress_percent(snapshot.percent);
                    app.set_progress_seekable(snapshot.seekable);
                    app.set_progress_text(SharedString::from(snapshot.label));
                }
                return;
            }

            // YouTube stream: compute the plan + stop here, then resolve the
            // restarted stream on a background thread.
            let Some(plan) = ctrl.prepare_stream_seek(percent) else {
                let snapshot = ctrl.progress_snapshot();
                drop(ctrl);
                if let Some(app) = weak.upgrade() {
                    app.set_progress_percent(snapshot.percent);
                    app.set_progress_seekable(snapshot.seekable);
                    app.set_progress_text(SharedString::from(snapshot.label));
                }
                return;
            };
            drop(ctrl);

            // Optimistic UI update so the bar tracks the drag immediately.
            if let Some(app) = weak.upgrade() {
                app.set_progress_percent(plan.percent);
                app.set_progress_text(SharedString::from(format!(
                    "{} / {}",
                    format_duration(plan.target_wall.as_secs_f64()),
                    format_duration(plan.wall_total.as_secs_f64())
                )));
                app.set_status_text(SharedString::from("Seeking..."));
            }

            let controller = Arc::clone(&controller);
            let weak = weak.clone();
            thread::spawn(move || {
                match ytdlp_stream_audio(&plan.stream_target, Some(plan.source_target)) {
                    Ok(stream) => {
                        let applied = controller
                            .lock()
                            .map(|mut ctrl| ctrl.apply_stream_seek(stream, &plan))
                            .unwrap_or(false);
                        if applied {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = weak.upgrade() {
                                    app.set_status_text(SharedString::from("Playing"));
                                }
                            });
                        }
                    }
                    Err(_) => {
                        let still_current = controller
                            .lock()
                            .map(|ctrl| {
                                ctrl.seek_generation == plan.generation
                                    && ctrl.current_stream_target.as_deref()
                                        == Some(plan.stream_target.as_str())
                            })
                            .unwrap_or(false);
                        if still_current {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = weak.upgrade() {
                                    app.set_status_text(SharedString::from(
                                        "Seek needs more buffer.",
                                    ));
                                }
                            });
                        }
                    }
                }
            });
        }
    });

    app.on_volume_changed({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move |volume| {
            if let Ok(mut controller) = controller.lock() {
                controller.set_volume(volume / 100.0);
                set_status(&weak, &format!("Volume: {}%", volume.round() as i32));
            } else {
                set_status(&weak, "Error: player controller lock poisoned");
            }
        }
    });

    app.on_repeat_cycle({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move || match controller.lock() {
            Ok(mut controller) => {
                let mode = controller.cycle_repeat();
                if let Some(app) = weak.upgrade() {
                    app.set_repeat_mode(mode.as_int());
                    app.set_status_text(SharedString::from(match mode {
                        RepeatMode::Off => "Repeat off",
                        RepeatMode::All => "Repeat all",
                        RepeatMode::One => "Repeat one",
                    }));
                }
            }
            Err(_) => set_status(&weak, "Repeat error: player controller lock poisoned"),
        }
    });

    app.on_output_select({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let output_devices = Arc::clone(&output_devices);
        move |index| {
            if output_devices.is_empty() {
                set_status(&weak, "No output devices found.");
                return;
            }
            let requested = index.max(0) as usize;
            if requested >= output_devices.len() {
                set_status(&weak, "Output device unavailable.");
                return;
            }

            match controller.lock() {
                Ok(mut controller) => {
                    let already_active = controller.selected_output_index == requested;
                    let index = match controller.select_output_device(requested, &output_devices) {
                        Ok(index) => index,
                        Err(error) => {
                            set_status(&weak, &format!("Output error: {error}"));
                            return;
                        }
                    };
                    let label = short_device_label(&output_devices[index].name);
                    if let Some(app) = weak.upgrade() {
                        app.set_output_label(SharedString::from(label.clone()));
                        app.set_output_items(output_model(&output_devices, index));
                    }
                    if !already_active {
                        reset_playback_ui(&weak, &format!("Output: {label}"));
                    }
                    set_status(&weak, &format!("Output: {label}"));
                }
                Err(_) => set_status(&weak, "Output error: player controller lock poisoned"),
            }
        }
    });

    app.on_focus_toggle({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move || {
            let enabled = weak
                .upgrade()
                .map(|app| app.get_focus_enabled())
                .unwrap_or(false);

            match controller.lock() {
                Ok(mut controller) => match controller.set_focus_enabled(enabled) {
                    Ok(()) => set_status(
                        &weak,
                        if enabled {
                            "Focus muffle on"
                        } else {
                            "Focus muffle off"
                        },
                    ),
                    Err(error) => set_status(&weak, &format!("Focus error: {error}")),
                },
                Err(_) => set_status(&weak, "Focus error: player controller lock poisoned"),
            }
        }
    });

    app.on_focus_intensity_changed({
        let controller = Arc::clone(&controller);
        move |value| {
            if let Ok(mut controller) = controller.lock() {
                controller.set_focus_intensity(value / 100.0);
            }
        }
    });

    app.on_tune_changed({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move |speed, reverb| {
            if let Ok(mut controller) = controller.lock() {
                controller.set_tune(speed, reverb);
                // A manual tweak diverges from any saved setting for this track.
                if let Some(app) = weak.upgrade() {
                    app.set_tune_saved(false);
                }
                let label = if (speed - 1.0).abs() < 0.001 && reverb <= 0.0 {
                    "Tune: off".to_string()
                } else {
                    format!(
                        "Tune: {:.0}% speed Â· {:.0}% reverb",
                        speed * 100.0,
                        reverb * 100.0
                    )
                };
                set_status(&weak, &label);
            } else {
                set_status(&weak, "Tune error: player controller lock poisoned");
            }
        }
    });

    app.on_tune_save({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move || {
            let saved = match controller.lock() {
                Ok(controller) => controller.current_tune_key().map(|key| {
                    let (speed, reverb) = controller.current_tune();
                    save_tune_for(&key, TuneSetting { speed, reverb });
                }),
                Err(_) => None,
            };
            if let Some(app) = weak.upgrade() {
                if saved.is_some() {
                    app.set_tune_saved(true);
                    app.set_status_text(SharedString::from("Tune saved for this track"));
                } else {
                    app.set_status_text(SharedString::from("Saving is only for downloaded tracks"));
                }
            }
        }
    });

    app.on_tune_clear({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        move || {
            if let Ok(controller) = controller.lock() {
                if let Some(key) = controller.current_tune_key() {
                    clear_tune_for(&key);
                }
            }
            if let Some(app) = weak.upgrade() {
                app.set_tune_saved(false);
                app.set_status_text(SharedString::from("Tune forgotten for this track"));
            }
        }
    });

    app.on_save_track({
        let weak = app.as_weak();
        move |url| {
            let target = url.to_string();
            if target.is_empty() {
                set_status(&weak, "No track selected.");
                return;
            }
            // Flash status bar and set saving message immediately
            if let Some(app) = weak.upgrade() {
                app.set_status_text(SharedString::from("Downloading..."));
                app.set_status_flash(true);
            }
            let weak2 = weak.clone();
            run_background(weak.clone(), "Downloaded.", move || {
                let saved = save_favorite_track(&target)?;
                // Reset flash after save completes
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak2.upgrade() {
                        app.set_status_flash(false);
                    }
                });
                Ok(AppUpdate {
                    now_title: None,
                    yt_results: None,
                    search_results: None,
                    now_playing_url: None,
                    status_text: Some(format!("Downloaded â€” {}", display_track_name(&saved))),
                    queue_context: None,
                })
            });
        }
    });

    app.on_download_selection({
        let weak = app.as_weak();
        move || {
            let urls: Vec<String> = match weak.upgrade() {
                Some(app) => selected_urls(&app)
                    .into_iter()
                    .filter(|u| is_youtube_url(u))
                    .collect(),
                None => return,
            };
            if urls.is_empty() {
                set_status(&weak, "Select Stream tracks to download.");
                return;
            }
            if let Some(app) = weak.upgrade() {
                app.set_status_text(SharedString::from(format!(
                    "Downloading {} track(s)...",
                    urls.len()
                )));
                app.set_status_flash(true);
                clear_row_selection(&app);
            }
            let weak2 = weak.clone();
            run_background(weak.clone(), "Downloads complete.", move || {
                let mut ok = 0;
                let mut fail = 0;
                for url in &urls {
                    match save_favorite_track(url) {
                        Ok(_) => ok += 1,
                        Err(_) => fail += 1,
                    }
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak2.upgrade() {
                        app.set_status_flash(false);
                    }
                });
                Ok(AppUpdate {
                    now_title: None,
                    yt_results: None,
                    search_results: None,
                    now_playing_url: None,
                    status_text: Some(if fail == 0 {
                        format!("Downloaded {ok} track(s).")
                    } else {
                        format!("Downloaded {ok}; {fail} failed.")
                    }),
                    queue_context: None,
                })
            });
        }
    });

    app.on_queue_remove({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |url| {
            let url_str = url.to_string();
            let source = weak
                .upgrade()
                .map(|a| a.get_source_mode().to_string())
                .unwrap_or_else(|| "youtube".to_string());

            // Decide what "remove" means based on what's being shown.
            let kind = match queue.lock() {
                Ok(q) => q.local_kind.clone(),
                Err(_) => LocalKind::None,
            };
            let mut status = "Removed from queue.".to_string();
            if source != "youtube" {
                match &kind {
                    LocalKind::Library => {
                        let slots = match queue.lock() {
                            Ok(mut q) => {
                                q.local_items.retain(|r| r.url != url_str);
                                q.local_items.clone()
                            }
                            Err(_) => Vec::new(),
                        };
                        status = match save_library_tracks(&slots) {
                            Ok(()) => "Removed from library.".to_string(),
                            Err(error) => {
                                format!(
                                    "Removed visually, but could not update library.json: {error}"
                                )
                            }
                        };
                    }
                    LocalKind::Downloads => {
                        // Delete the actual file from disk and report if it fails.
                        // Guard: only ever delete inside the downloads folder.
                        let target = PathBuf::from(&url_str);
                        status = if !path_within_downloads(&target) {
                            "Skipped â€” not a managed download.".to_string()
                        } else {
                            match fs::remove_file(&target) {
                                Ok(()) => "Deleted download.".to_string(),
                                Err(error) => format!("Could not delete (in use?): {error}"),
                            }
                        };
                        let slots = favorite_slots().unwrap_or_default();
                        if let Ok(mut q) = queue.lock() {
                            q.local_items = slots;
                        }
                    }
                    LocalKind::Playlist(name) => {
                        // Remove the track from the saved playlist file.
                        if let Ok(mut pl) = load_playlist(name) {
                            pl.tracks.retain(|t| t.url != url_str);
                            let _ = save_playlist(name, &pl.tracks);
                            if let Ok(mut q) = queue.lock() {
                                q.local_items = pl.tracks.clone();
                            }
                        }
                        status = "Removed from playlist.".to_string();
                    }
                    LocalKind::None => {
                        if let Ok(mut q) = queue.lock() {
                            q.local_items.retain(|r| r.url != url_str);
                        }
                    }
                }
            }

            let result = queue.lock().map(|mut q| {
                if source == "youtube" {
                    q.yt_items.retain(|r| r.url != url_str);
                }
                q.results.retain(|r| r.url != url_str);
                q.current_index = q.results.iter().position(|r| r.url == q.now_playing_url);
                let list = if source == "youtube" {
                    q.yt_items.clone()
                } else {
                    q.local_items.clone()
                };
                (list, q.now_playing_url.clone())
            });
            match result {
                Ok((all, playing)) => {
                    if let Some(app) = weak.upgrade() {
                        app.set_queue_items(queue_model(&all, &playing));
                        app.set_selected_count(0);
                        app.set_status_text(SharedString::from(status));
                    }
                }
                Err(_) => set_status(&weak, "Queue error."),
            }
        }
    });

    // â”€â”€ Playlists â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    app.on_queue_clear({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        move || {
            if let Ok(mut controller) = controller.lock() {
                controller.stop();
            }
            match queue.lock() {
                Ok(mut q) => {
                    q.clear();
                    q.yt_items.clear();
                    q.now_playing_url.clear();
                }
                Err(_) => {
                    set_status(&weak, "Queue error.");
                    return;
                }
            }
            if let Some(app) = weak.upgrade() {
                app.set_queue_items(queue_model(&[], ""));
                app.set_selected_count(0);
                app.set_search_results(search_model(&[]));
                app.set_now_title(SharedString::from("Nothing playing"));
                app.set_playback_active(false);
                app.set_playback_paused(false);
                app.set_progress_percent(0.0);
                app.set_progress_text(SharedString::from("0:00 / 0:00"));
                app.set_status_text(SharedString::from("Queue cleared."));
            }
        }
    });

    app.on_create_playlist({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                app.set_naming_playlist(true);
                app.set_naming_whole_queue(false);
                app.set_name_flash(true);
                app.set_input_text(SharedString::from(""));
                app.set_status_text(SharedString::from("Name your playlist, then press Enter."));
            }
        }
    });

    app.on_create_queue_playlist({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            let has_tracks = match queue.lock() {
                Ok(q) => !q.yt_items.is_empty(),
                Err(_) => false,
            };
            if !has_tracks {
                set_status(&weak, "Queue is empty.");
                return;
            }
            if let Some(app) = weak.upgrade() {
                app.set_source_mode(SharedString::from("youtube"));
                app.set_naming_playlist(true);
                app.set_naming_whole_queue(true);
                app.set_name_flash(true);
                app.set_input_text(SharedString::from(""));
                app.set_status_text(SharedString::from(
                    "Name this queue playlist, then press Enter.",
                ));
            }
        }
    });

    app.on_cancel_naming({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                app.set_naming_playlist(false);
                app.set_naming_whole_queue(false);
                app.set_name_flash(false);
                app.set_input_text(SharedString::from(""));
                app.set_status_text(SharedString::from("Ready"));
            }
        }
    });

    app.on_save_playlist({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |name| {
            let name = name.trim().to_string();
            if name.is_empty() {
                set_status(&weak, "Type a name for the playlist first.");
                return;
            }
            let (selected_urls, source, whole_queue) = match weak.upgrade() {
                Some(app) => (
                    selected_urls(&app),
                    app.get_source_mode().to_string(),
                    app.get_naming_whole_queue(),
                ),
                None => return,
            };
            if selected_urls.is_empty() && !whole_queue {
                set_status(&weak, "Tick the tracks you want with the checkboxes first.");
                return;
            }
            // Resolve to underlying slots (clean titles), preserving list order.
            let tracks: Vec<YtResultSlot> = match queue.lock() {
                Ok(q) => {
                    let list = if source == "youtube" {
                        &q.yt_items
                    } else {
                        &q.local_items
                    };
                    if whole_queue {
                        list.iter()
                            .filter(|slot| !slot.url.is_empty())
                            .cloned()
                            .collect()
                    } else {
                        list.iter()
                            .filter(|slot| selected_urls.contains(&slot.url))
                            .cloned()
                            .collect()
                    }
                }
                Err(_) => {
                    set_status(&weak, "Error: queue lock poisoned");
                    return;
                }
            };
            if tracks.is_empty() {
                set_status(&weak, "Could not resolve the selected tracks.");
                return;
            }
            match save_playlist(&name, &tracks) {
                Ok(()) => {
                    if let Some(app) = weak.upgrade() {
                        app.set_naming_playlist(false);
                        app.set_naming_whole_queue(false);
                        app.set_name_flash(false);
                        app.set_input_text(SharedString::from(""));
                        clear_row_selection(&app);
                        app.set_playlist_items(playlist_model(
                            &list_playlists().unwrap_or_default(),
                        ));
                        app.set_status_text(SharedString::from(format!(
                            "Saved playlist '{name}' ({} tracks)",
                            tracks.len()
                        )));
                    }
                }
                Err(error) => set_status(&weak, &format!("Error saving playlist: {error}")),
            }
        }
    });

    app.on_toggle_select({
        let weak = app.as_weak();
        move |url| {
            if let Some(app) = weak.upgrade() {
                let model = app.get_queue_items();
                let mut count = 0;
                for i in 0..model.row_count() {
                    if let Some(mut row) = model.row_data(i) {
                        if row.url == url {
                            row.selected = !row.selected;
                            model.set_row_data(i, row.clone());
                        }
                        if row.selected {
                            count += 1;
                        }
                    }
                }
                app.set_selected_count(count);
            }
        }
    });

    app.on_delete_selection({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move || {
            let urls = match weak.upgrade() {
                Some(app) => selected_urls(&app),
                None => return,
            };
            if urls.is_empty() {
                return;
            }
            let kind = match queue.lock() {
                Ok(q) => q.local_kind.clone(),
                Err(_) => LocalKind::None,
            };

            let (slots, msg) = match kind {
                LocalKind::Library => {
                    let mut slots = match queue.lock() {
                        Ok(q) => q.local_items.clone(),
                        Err(_) => Vec::new(),
                    };
                    let before = slots.len();
                    slots.retain(|t| !urls.contains(&t.url));
                    let removed = before - slots.len();
                    let msg = match save_library_tracks(&slots) {
                        Ok(()) => {
                            format!("Removed {removed} track(s) from the library index.")
                        }
                        Err(error) => {
                            format!(
                                "Removed {removed} visually, but could not update library.json: {error}"
                            )
                        }
                    };
                    (slots, msg)
                }
                LocalKind::Playlist(name) => {
                    // Remove the checked tracks from the saved playlist file
                    // (does NOT delete any downloaded audio).
                    let mut playlist = load_playlist(&name).unwrap_or(Playlist {
                        name: name.clone(),
                        tracks: Vec::new(),
                    });
                    let before = playlist.tracks.len();
                    playlist.tracks.retain(|t| !urls.contains(&t.url));
                    let removed = before - playlist.tracks.len();
                    let _ = save_playlist(&name, &playlist.tracks);
                    (
                        playlist.tracks,
                        format!("Removed {removed} track(s) from \"{name}\"."),
                    )
                }
                LocalKind::Downloads => {
                    // Downloads: delete the actual files from disk â€” but only ever
                    // files inside the managed downloads folder (guard).
                    let mut deleted = 0;
                    let mut failed = 0;
                    for url in &urls {
                        let target = PathBuf::from(url);
                        if !path_within_downloads(&target) {
                            failed += 1;
                            continue;
                        }
                        match fs::remove_file(&target) {
                            Ok(()) => deleted += 1,
                            Err(_) => failed += 1,
                        }
                    }
                    let slots = favorite_slots().unwrap_or_default();
                    let msg = if failed == 0 {
                        format!("Deleted {deleted} download(s).")
                    } else {
                        format!("Deleted {deleted}; {failed} kept (in use or not a download).")
                    };
                    (slots, msg)
                }
                LocalKind::None => (Vec::new(), "Nothing to remove.".to_string()),
            };

            let playing = match queue.lock() {
                Ok(mut q) => {
                    q.local_items = slots.clone();
                    q.now_playing_url.clone()
                }
                Err(_) => String::new(),
            };
            if let Some(app) = weak.upgrade() {
                app.set_queue_items(queue_model(&slots, &playing));
                app.set_selected_count(0);
                app.set_status_text(SharedString::from(msg));
            }
        }
    });

    app.on_open_playlist_picker({
        let weak = app.as_weak();
        move || {
            if let Some(app) = weak.upgrade() {
                // Refresh the saved-playlist list so the picker is current.
                app.set_playlist_items(playlist_model(&list_playlists().unwrap_or_default()));
                app.set_picker_open(true);
            }
        }
    });

    app.on_add_to_playlist({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |name| {
            let name = name.to_string();
            let (urls, source) = match weak.upgrade() {
                Some(app) => (selected_urls(&app), app.get_source_mode().to_string()),
                None => return,
            };
            if urls.is_empty() {
                set_status(&weak, "Nothing selected.");
                return;
            }
            let new_tracks: Vec<YtResultSlot> = match queue.lock() {
                Ok(q) => {
                    let list = if source == "youtube" {
                        &q.yt_items
                    } else {
                        &q.local_items
                    };
                    list.iter()
                        .filter(|s| urls.contains(&s.url))
                        .cloned()
                        .collect()
                }
                Err(_) => return,
            };
            // Append to the existing playlist, skipping tracks already in it.
            let mut playlist = load_playlist(&name).unwrap_or(Playlist {
                name: name.clone(),
                tracks: Vec::new(),
            });
            let mut added = 0;
            for track in new_tracks {
                if !playlist.tracks.iter().any(|t| t.url == track.url) {
                    playlist.tracks.push(track);
                    added += 1;
                }
            }
            match save_playlist(&name, &playlist.tracks) {
                Ok(()) => {
                    if let Some(app) = weak.upgrade() {
                        clear_row_selection(&app);
                        app.set_playlist_items(playlist_model(
                            &list_playlists().unwrap_or_default(),
                        ));
                        app.set_status_text(SharedString::from(if added == 0 {
                            format!("Those tracks are already in '{name}'.")
                        } else {
                            format!("Added {added} track(s) to '{name}'.")
                        }));
                    }
                }
                Err(error) => set_status(&weak, &format!("Error updating playlist: {error}")),
            }
        }
    });

    app.on_playlist_delete({
        let weak = app.as_weak();
        move |name| {
            let name = name.to_string();
            match delete_playlist(&name) {
                Ok(()) => {
                    let playlists = list_playlists().unwrap_or_default();
                    if let Some(app) = weak.upgrade() {
                        app.set_local_tab(1);
                        app.set_local_show_playlists(true);
                        app.set_local_list_label(SharedString::from("PLAYLISTS"));
                        app.set_playlist_items(playlist_model(&playlists));
                        app.set_queue_items(queue_model(&[], ""));
                        app.set_selected_count(0);
                        app.set_status_text(SharedString::from(format!(
                            "Deleted playlist: {name}"
                        )));
                    }
                }
                Err(error) => set_status(&weak, &format!("Error deleting playlist: {error}")),
            }
        }
    });

    app.on_playlist_open({
        let weak = app.as_weak();
        let queue = Arc::clone(&queue);
        move |name| {
            let name = name.to_string();
            let playlist = match load_playlist(&name) {
                Ok(p) => p,
                Err(error) => {
                    set_status(&weak, &format!("Error opening playlist: {error}"));
                    return;
                }
            };
            if playlist.tracks.is_empty() {
                set_status(&weak, "That playlist is empty.");
                return;
            }
            // Load the playlist into the Local list â€” but DON'T play or stop the
            // current track. The user picks a track to start it.
            let playing = {
                let mut q = match queue.lock() {
                    Ok(q) => q,
                    Err(_) => {
                        set_status(&weak, "Error: queue lock poisoned");
                        return;
                    }
                };
                q.local_items = playlist.tracks.clone();
                q.local_kind = LocalKind::Playlist(name.clone());
                q.now_playing_url.clone()
            };
            if let Some(app) = weak.upgrade() {
                app.set_playlist_dropdown_open(false);
                app.set_local_tab(1);
                app.set_local_show_playlists(true);
                app.set_local_list_label(SharedString::from(playlist.name.clone()));
                app.set_queue_items(queue_model(&playlist.tracks, &playing));
                app.set_selected_count(0);
                app.set_status_text(SharedString::from(format!(
                    "{} â€” pick a track to play.",
                    playlist.name
                )));
            }
        }
    });

    app.on_open_downloads_folder({
        let weak = app.as_weak();
        move || match favorites_dir() {
            Ok(dir) => {
                let _ = fs::create_dir_all(&dir);
                #[cfg(windows)]
                {
                    let _ = hidden_command("explorer").arg(&dir).spawn();
                }
                set_status(&weak, "Opened downloads folder.");
            }
            Err(_) => set_status(&weak, "Could not open downloads folder."),
        }
    });

    // Click a track in the queue/list: play it, making the current tab's list
    // the playback queue (so prev/next walk that list).
    app.on_play_queue_row({
        let weak = app.as_weak();
        let controller = Arc::clone(&controller);
        let queue = Arc::clone(&queue);
        move |url| {
            let target = url.to_string();
            if target.trim().is_empty() {
                return;
            }
            let source = weak
                .upgrade()
                .map(|a| a.get_source_mode().to_string())
                .unwrap_or_else(|| "youtube".to_string());
            let list = {
                let mut q = match queue.lock() {
                    Ok(q) => q,
                    Err(_) => {
                        set_status(&weak, "Error: queue lock poisoned");
                        return;
                    }
                };
                let list = if source == "youtube" {
                    q.yt_items.clone()
                } else {
                    q.local_items.clone()
                };
                let context = if source == "youtube" {
                    QueueContext::Stream
                } else {
                    current_local_context(&q)
                };
                q.play_from_context(list.clone(), &target, context);
                list
            };
            if let Some(app) = weak.upgrade() {
                app.set_queue_items(queue_model(&list, &target));
                app.set_selected_count(0);
                // Instant now-playing feedback from the clicked row (strip the
                // "N  " index prefix); the background task refines it shortly.
                let display = {
                    let model = app.get_queue_items();
                    (0..model.row_count())
                        .filter_map(|i| model.row_data(i))
                        .find(|row| row.url == target)
                        .map(|row| row.title.to_string())
                        .unwrap_or_default()
                };
                let title = display
                    .split_once("  ")
                    .map(|(_, title)| title)
                    .unwrap_or(display.as_str());
                if !title.is_empty() {
                    app.set_now_title(SharedString::from(title));
                }
            }
            stop_current_for_new_track(&weak, &controller, "Opening track...");
            let gen = next_play_gen(&queue);
            if is_youtube_url(&target) {
                play_queued_track(
                    weak.clone(),
                    Arc::clone(&controller),
                    Arc::clone(&queue),
                    QueuedTrack {
                        title: String::new(),
                        url: target.clone(),
                    },
                    gen,
                    "Playing",
                );
            } else {
                play_local_tracks(
                    weak.clone(),
                    Arc::clone(&controller),
                    Arc::clone(&queue),
                    vec![PathBuf::from(&target)],
                    gen,
                    "Downloaded",
                );
            }
        }
    });
}

struct AppUpdate {
    now_title: Option<String>,
    yt_results: Option<Vec<YtResultSlot>>,
    search_results: Option<Vec<YtResultSlot>>,
    now_playing_url: Option<String>,
    status_text: Option<String>,
    queue_context: Option<QueueContext>,
}

// Fisher-Yates shuffle (xorshift64, time-seeded â€” no rand dependency).
// `keep_first`, if present in the list, is moved to the front so the
// currently-playing track stays current and the next track is random.
fn shuffle_slots(list: &mut [YtResultSlot], keep_first: &str) {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        | 1;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let n = list.len();
    for i in (1..n).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        list.swap(i, j);
    }
    if !keep_first.is_empty() {
        if let Some(pos) = list.iter().position(|r| r.url == keep_first) {
            list.swap(0, pos);
        }
    }
}

fn yt_slot_from_entry(result: &YtEntry) -> Option<YtResultSlot> {
    let raw_url = result.webpage_url.as_deref().or(result.url.as_deref())?;
    let url = normalize_youtube_entry_url(raw_url);
    Some(YtResultSlot {
        title: clean_title(result.title.as_deref().unwrap_or("Untitled")),
        url,
    })
}

fn normalize_youtube_entry_url(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("https://www.youtube.com/watch?v={raw}")
    }
}

// URLs of checkbox-selected rows in the currently displayed list.
fn selected_urls(app: &AppWindow) -> Vec<String> {
    let model = app.get_queue_items();
    (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .filter(|row| row.selected)
        .map(|row| row.url.to_string())
        .collect()
}

// Untick every row in the visible list (without a full rebuild) and zero the count.
fn clear_row_selection(app: &AppWindow) {
    let model = app.get_queue_items();
    for i in 0..model.row_count() {
        if let Some(mut row) = model.row_data(i) {
            if row.selected {
                row.selected = false;
                model.set_row_data(i, row);
            }
        }
    }
    app.set_selected_count(0);
}

fn queue_model(results: &[YtResultSlot], playing_url: &str) -> ModelRc<QueueRow> {
    let downloads = download_file_names();
    let rows = results
        .iter()
        .enumerate()
        .map(|(index, result)| QueueRow {
            title: SharedString::from(format!("{}  {}", index + 1, result.title)),
            url: SharedString::from(result.url.clone()),
            active: !result.url.is_empty() && result.url == playing_url,
            is_remote: is_youtube_url(&result.url),
            downloaded: is_url_downloaded(&result.url, &downloads),
            selected: false,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn search_model(results: &[YtResultSlot]) -> ModelRc<QueueRow> {
    let rows = results
        .iter()
        .map(|result| QueueRow {
            title: SharedString::from(result.title.clone()),
            url: SharedString::from(result.url.clone()),
            active: false,
            is_remote: is_youtube_url(&result.url),
            downloaded: false,
            selected: false,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn playlist_model(playlists: &[Playlist]) -> ModelRc<QueueRow> {
    let rows = playlists
        .iter()
        .map(|playlist| QueueRow {
            title: SharedString::from(format!(
                "{}  ({} tracks)",
                playlist.name,
                playlist.tracks.len()
            )),
            // url carries the playlist name for open/delete callbacks
            url: SharedString::from(playlist.name.clone()),
            active: false,
            is_remote: false,
            downloaded: false,
            selected: false,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn output_model(devices: &[OutputDeviceInfo], active_index: usize) -> ModelRc<OutputRow> {
    let rows = devices
        .iter()
        .enumerate()
        .map(|(index, device)| OutputRow {
            title: SharedString::from(device.name.clone()),
            index: index as i32,
            active: index == active_index,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

fn clean_title(title: &str) -> String {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let shortened = chars.by_ref().take(69).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else {
        normalized
    }
}

// Push search results to the UI from a background thread; results stream in
// one at a time so the panel expands as yt-dlp finds them (feels snappier).
fn push_search_results(
    weak: &slint::Weak<AppWindow>,
    queue: &Arc<Mutex<AppQueue>>,
    gen: u64,
    slots: Vec<YtResultSlot>,
    status: Option<String>,
) {
    let weak = weak.clone();
    let queue = Arc::clone(queue);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            if search_gen_is_stale(&queue, gen) || app.get_source_mode() != "youtube" {
                return;
            }
            app.set_search_results(search_model(&slots));
            if let Some(status) = status {
                app.set_status_text(SharedString::from(status));
            }
        }
    });
}

fn start_youtube_search(weak: slint::Weak<AppWindow>, queue: Arc<Mutex<AppQueue>>, query: String) {
    let query = query.trim().to_string();
    if query.is_empty() {
        cancel_search(&queue);
        set_status(&weak, "Ready");
        return;
    }

    let gen = next_search_gen(&queue);
    {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                app.set_search_results(search_model(&[]));
            }
        });
    }
    set_status(&weak, "Searching...");
    thread::spawn(move || {
        let args = vec![
            "--dump-json".to_string(),
            "--flat-playlist".to_string(),
            "--no-warnings".to_string(),
            format!("ytsearch10:{query}"),
        ];
        let mut child = match ytdlp_spawn(&args) {
            Ok(child) => child,
            Err(error) => {
                // On a worker thread â€” must marshal UI updates onto the event loop.
                set_status_async(&weak, error_status_text(error.as_ref()));
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            set_status_async(&weak, "Search failed: no output from yt-dlp.".to_string());
            return;
        };

        let mut slots: Vec<YtResultSlot> = Vec::new();
        for line in BufReader::new(stdout).lines() {
            if search_gen_is_stale(&queue, gen) {
                let _ = child.kill();
                return;
            }
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<YtEntry>(&line) {
                if let Some(slot) = yt_slot_from_entry(&entry) {
                    slots.push(slot);
                    // Stream the partial list to the UI as it grows.
                    push_search_results(&weak, &queue, gen, slots.clone(), None);
                }
            }
        }
        let _ = child.wait();
        if search_gen_is_stale(&queue, gen) {
            return;
        }

        let status = if slots.is_empty() {
            "No results.".to_string()
        } else {
            "Pick a result.".to_string()
        };
        push_search_results(&weak, &queue, gen, slots, Some(status));
    });
}

fn add_track_to_queue(weak: slint::Weak<AppWindow>, queue: Arc<Mutex<AppQueue>>, input: String) {
    let target = input.trim().to_string();
    if target.is_empty() {
        set_status(&weak, "Enter a search or URL first.");
        return;
    }

    set_status(&weak, "Adding to queue...");
    run_background(weak, "Added to queue.", move || {
        // Resolve a title + canonical URL for the single track
        let slot = if is_youtube_url(&target) {
            // Direct URL â€” fetch title via metadata
            let yt_target = yt_target(&target);
            let meta = yt_metadata(&yt_target)?;
            YtResultSlot {
                title: meta.title.unwrap_or_else(|| "Unknown".to_string()),
                url: target.clone(),
            }
        } else {
            // Search query â€” take first result
            let results = yt_search_results(&target, 1)?;
            let entry = results.into_iter().next().ok_or("No results found")?;
            yt_slot_from_entry(&entry).ok_or("Could not resolve track")?
        };

        let title = slot.title.clone();
        let slots = {
            let mut q = queue.lock().map_err(|_| "queue lock poisoned")?;
            if !q.yt_items.iter().any(|existing| existing.url == slot.url) {
                q.yt_items.push(slot);
            }
            q.yt_items.clone()
        };

        Ok(AppUpdate {
            now_title: None,
            yt_results: Some(slots),
            search_results: Some(Vec::new()),
            now_playing_url: None,
            status_text: Some(format!("Queued: {title}")),
            queue_context: Some(QueueContext::Stream),
        })
    });
}

// â”€â”€ Playlist import: Spotify / Apple Music link -> resolve via Stream â”€â”€â”€â”€â”€â”€â”€â”€â”€
// We never touch those services' audio (DRM); we only read the PUBLIC track list
// (title + artist) from the page, then resolve each track to a Stream (YouTube)
// source with yt-dlp â€” the same isolation as a normal search. Best-effort and
// fragile by design: if a page format changes the import fails gracefully and
// local playback is unaffected.

// Resolve imported tracks concurrently â€” each yt-dlp search is its own process,
// so the per-track ~1-3s startup dominated a sequential import. A small pool
// cuts wall time ~Nx. Transient cost: up to this many yt-dlp processes during an
// import only (a few hundred MB spike), back to baseline once done.
const IMPORT_WORKERS: usize = 4;

fn set_status_async(weak: &slint::Weak<AppWindow>, msg: String) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            update_helper_repair_prompt(&app, &msg);
            app.set_status_text(SharedString::from(msg));
        }
    });
}

fn set_import_active_async(weak: &slint::Weak<AppWindow>, active: bool) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.set_import_active(active);
        }
    });
}

// â”€â”€ Update check â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// Lightweight, opt-out-by-dismiss: on startup, ask GitHub for the latest release
// and, if it's newer than this build, surface a banner. Network-failure-silent.
//
// SECURITY: remote update metadata (the GitHub release JSON) is used ONLY to read
// a version number. It is NEVER trusted as a navigation target â€” we never open a
// URL that came from the API/network. The banner always opens the single
// hardcoded, validated `UPDATE_PAGE_URL` below, so a spoofed/compromised/MITM'd
// API response cannot redirect the user to an attacker-controlled location. We
// also never auto-download or auto-run an installer.

/// Off-thread startup check; flips the banner on only if a newer build exists.
fn check_for_update(weak: slint::Weak<AppWindow>) {
    thread::spawn(move || {
        let Some((tag, remote)) = fetch_latest_release() else {
            return;
        };
        let current = parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0));
        if remote <= current {
            return;
        }
        let label = format!(
            "Version {} is available",
            tag.trim_start_matches(|c: char| !c.is_ascii_digit())
        );
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                app.set_update_label(SharedString::from(label));
                app.set_update_available(true);
            }
        });
    });
}

/// Read a Spotify/Apple playlist link, resolve each track to a Stream source,
/// build the queue (incrementally, with progress), and start playing the first.
fn import_playlist_link(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
    import_cancel: Arc<AtomicBool>,
    url: String,
) {
    set_status_async(&weak, "Reading playlistâ€¦".to_string());
    set_import_active_async(&weak, true);
    thread::spawn(move || {
        let queries = match fetch_playlist_tracks(&url) {
            Ok(q) => q,
            Err(error) => {
                set_status_async(&weak, format!("Import failed: {error}"));
                set_import_active_async(&weak, false);
                return;
            }
        };

        let total = queries.len();
        if import_cancel.load(Ordering::Relaxed) {
            set_status_async(&weak, "Import cancelled.".to_string());
            set_import_active_async(&weak, false);
            return;
        }
        if let Ok(mut q) = queue.lock() {
            q.clear();
            q.yt_items.clear();
        }
        // Clear the displayed lists up front.
        {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    app.set_search_results(search_model(&[]));
                    app.set_queue_items(queue_model(&[], ""));
                }
            });
        }

        // Resolve in parallel with a bounded pool. Results are placed by index so
        // the queue keeps the playlist's order regardless of completion order;
        // each completion mirrors the ordered list into yt_items + the UI.
        let queries = Arc::new(queries);
        let results: Arc<Mutex<Vec<Option<YtResultSlot>>>> =
            Arc::new(Mutex::new(vec![None; total]));
        let next_index = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..IMPORT_WORKERS.min(total.max(1)) {
            let queries = Arc::clone(&queries);
            let results = Arc::clone(&results);
            let next_index = Arc::clone(&next_index);
            let done = Arc::clone(&done);
            let import_cancel = Arc::clone(&import_cancel);
            let queue = Arc::clone(&queue);
            let weak = weak.clone();
            handles.push(thread::spawn(move || loop {
                if import_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next_index.fetch_add(1, Ordering::Relaxed);
                if i >= queries.len() {
                    break;
                }
                let slot = yt_search_results(&queries[i], 1)
                    .ok()
                    .and_then(|r| r.into_iter().next())
                    .and_then(|entry| yt_slot_from_entry(&entry));
                if let Ok(mut res) = results.lock() {
                    res[i] = slot;
                }
                let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                // Ordered list of what's resolved so far (gaps from None skipped).
                let ordered: Vec<YtResultSlot> = match results.lock() {
                    Ok(res) => res.iter().flatten().cloned().collect(),
                    Err(_) => Vec::new(),
                };
                if let Ok(mut q) = queue.lock() {
                    q.yt_items = ordered.clone();
                }
                {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            app.set_queue_items(queue_model(&ordered, ""));
                            app.set_selected_count(0);
                        }
                    });
                }
                set_status_async(&weak, format!("Resolving {finished}/{total}â€¦"));
            }));
        }
        for handle in handles {
            let _ = handle.join();
        }

        let resolved = match results.lock() {
            Ok(res) => res.iter().flatten().count(),
            Err(_) => 0,
        };

        // Keep the queue on the Stream tab and set the playback position to the
        // first track, then start it.
        let first = match queue.lock() {
            Ok(mut q) => {
                let first = q.yt_items.iter().find(|s| !s.url.is_empty()).cloned();
                if let Some(f) = &first {
                    let list = q.yt_items.clone();
                    q.play_from_context(list, &f.url, QueueContext::Stream);
                }
                first
            }
            Err(_) => None,
        };

        if let Some(slot) = first {
            if import_cancel.load(Ordering::Relaxed) {
                set_status_async(
                    &weak,
                    format!("Import cancelled after {resolved} of {total} tracks."),
                );
                set_import_active_async(&weak, false);
                return;
            }
            let weak = weak.clone();
            let controller = Arc::clone(&controller);
            let queue = Arc::clone(&queue);
            let _ = slint::invoke_from_event_loop(move || {
                let gen = next_play_gen(&queue);
                play_track_by_url(
                    weak,
                    controller,
                    queue,
                    QueuedTrack {
                        title: slot.title,
                        url: slot.url,
                    },
                    gen,
                    "Playing",
                );
            });
        }

        if import_cancel.load(Ordering::Relaxed) {
            set_status_async(
                &weak,
                format!("Import cancelled after {resolved} of {total} tracks."),
            );
            set_import_active_async(&weak, false);
            return;
        }

        set_status_async(&weak, format!("Imported {resolved} of {total} tracks."));
        set_import_active_async(&weak, false);
    });
}

// Pull the 11-ish char video id out of a YouTube URL.
fn youtube_video_id(url: &str) -> Option<String> {
    let take_id = |rest: &str| -> Option<String> {
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if id.is_empty() {
            None
        } else {
            Some(id)
        }
    };
    if let Some(pos) = url.find("v=") {
        if let Some(id) = take_id(&url[pos + 2..]) {
            return Some(id);
        }
    }
    if let Some(pos) = url.find("youtu.be/") {
        if let Some(id) = take_id(&url[pos + 9..]) {
            return Some(id);
        }
    }
    None
}

// True if a YouTube track has already been downloaded (matched by video id in
// the downloads folder, where files are named "... [id].mp3").
fn is_url_downloaded(url: &str, download_names: &[String]) -> bool {
    if !is_youtube_url(url) {
        return false;
    }
    match youtube_video_id(url) {
        Some(id) => {
            let marker = format!("[{id}]");
            download_names.iter().any(|name| name.contains(&marker))
        }
        None => false,
    }
}

fn download_file_names() -> Vec<String> {
    favorite_audio_files()
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.file_name().and_then(OsStr::to_str).map(String::from))
        .collect()
}

fn play_queue_neighbor(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
    direction: QueueDirection,
) {
    let track = match queue.lock() {
        Ok(mut queue) => queue.select_neighbor(direction),
        Err(_) => {
            set_status(&weak, "Error: queue lock poisoned");
            return;
        }
    };

    let Some(track) = track else {
        set_status(&weak, "No queued result in that direction.");
        return;
    };

    stop_current_for_new_track(&weak, &controller, &format!("Loading {}...", track.title));
    let gen = next_play_gen(&queue);
    play_track_by_url(weak, controller, queue, track, gen, "Playing");
}

// Auto-advance when a track ends (driven by the progress timer). Plays the next
// queue item; wraps to the top under repeat-all, stops cleanly under repeat-off.
fn advance_to_next(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
) {
    let mode = match controller.lock() {
        Ok(ctrl) => ctrl.repeat_mode(),
        Err(_) => RepeatMode::Off,
    };
    let track = match queue.lock() {
        Ok(mut q) => match q.select_neighbor(QueueDirection::Next) {
            Some(t) => Some(t),
            None if mode == RepeatMode::All => q.select_index(0),
            None => None,
        },
        Err(_) => None,
    };

    let Some(track) = track else {
        // End of the queue with repeat off â€” stop cleanly.
        if let Ok(mut controller) = controller.lock() {
            controller.stop();
        }
        reset_playback_ui(&weak, "End of queue.");
        return;
    };

    stop_current_for_new_track(&weak, &controller, "Next track...");
    let gen = next_play_gen(&queue);
    play_track_by_url(weak, controller, queue, track, gen, "Playing");
}

fn restart_queue_from_start(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
) {
    let source = weak
        .upgrade()
        .map(|app| app.get_source_mode().to_string())
        .unwrap_or_else(|| "youtube".to_string());

    let (track, list, playing) = match queue.lock() {
        Ok(mut q) => {
            if q.results.is_empty() {
                let list = if source == "youtube" {
                    q.yt_items.clone()
                } else {
                    q.local_items.clone()
                };
                let Some(first) = list.first() else {
                    set_status(&weak, "Pick a track first.");
                    return;
                };
                let url = first.url.clone();
                let context = if source == "youtube" {
                    QueueContext::Stream
                } else {
                    current_local_context(&q)
                };
                q.play_from_context(list, &url, context);
            }

            let Some(track) = q.select_index(0) else {
                set_status(&weak, "Pick a track first.");
                return;
            };
            q.now_playing_url = track.url.clone();
            let list = if source == "youtube" {
                q.yt_items.clone()
            } else {
                q.results.clone()
            };
            (track, list, q.now_playing_url.clone())
        }
        Err(_) => {
            set_status(&weak, "Error: queue lock poisoned");
            return;
        }
    };

    if let Some(app) = weak.upgrade() {
        app.set_queue_items(queue_model(&list, &playing));
        app.set_selected_count(0);
    }

    stop_current_for_new_track(&weak, &controller, "Restarting queue...");
    let gen = next_play_gen(&queue);
    play_track_by_url(weak, controller, queue, track, gen, "Playing");
}

// Play any queue item by URL: stream it if it's a YouTube link, otherwise play
// the local file. Keeps the now-playing highlight and title in sync either way.
fn play_track_by_url(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
    track: QueuedTrack,
    gen: u64,
    detail: &'static str,
) {
    if is_youtube_url(&track.url) {
        play_queued_track(weak, controller, queue, track, gen, detail);
        return;
    }

    // Local file.
    let path = PathBuf::from(&track.url);
    let (list, playing, context) = match queue.lock() {
        Ok(mut q) => {
            q.now_playing_url = track.url.clone();
            (
                q.results.clone(),
                q.now_playing_url.clone(),
                q.playback_context.clone(),
            )
        }
        Err(_) => (Vec::new(), track.url.clone(), QueueContext::Stream),
    };
    if let Some(app) = weak.upgrade() {
        if visible_context_matches(&app, &context) {
            app.set_queue_items(queue_model(&list, &playing));
            app.set_selected_count(0);
        }
    }
    play_local_tracks(weak, controller, queue, vec![path], gen, detail);
}

fn play_queued_track(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
    track: QueuedTrack,
    gen: u64,
    _detail: &'static str,
) {
    // Streams are never saved per-track â€” hide the save option.
    if let Some(app) = weak.upgrade() {
        app.set_tune_can_save(false);
        app.set_tune_saved(false);
    }
    let track_url = track.url.clone();
    run_background(weak, "Streaming.", move || {
        let (title, duration, stream_target, stream) = prepare_youtube_stream(&track.url)?;
        // A newer track was requested while yt-dlp resolved â€” abandon quietly.
        if play_gen_is_stale(&queue, gen) {
            return Ok(stale_update());
        }
        controller
            .lock()
            .map_err(|_| "player controller lock poisoned")?
            .play_stream(stream, duration, stream_target, clean_title(&title))?;

        let (results, playing, context) = {
            let mut queue = queue.lock().map_err(|_| "queue lock poisoned")?;
            if queue.play_gen != gen {
                return Ok(stale_update());
            }
            if queue.results.is_empty() || !queue.results.iter().any(|slot| slot.url == track_url) {
                // No queue context yet (e.g. a single pasted link) â€” seed from the
                // Stream list as the sensible default.
                let list = queue.yt_items.clone();
                queue.play_from(list, &track_url);
            }
            queue.now_playing_url = track_url.clone();
            // Repaint from `results` â€” the authoritative playback queue the caller
            // set (Stream rows, Local downloads, or a Local playlist). Using
            // yt_items here overwrote the Local view with the Stream queue.
            (
                queue.results.clone(),
                queue.now_playing_url.clone(),
                queue.playback_context.clone(),
            )
        };
        Ok(AppUpdate {
            now_title: Some(clean_title(&title)),
            yt_results: Some(results),
            search_results: None,
            now_playing_url: Some(playing),
            status_text: Some("Playing.".to_string()),
            queue_context: Some(context),
        })
    });
}

fn run_background<F>(weak: slint::Weak<AppWindow>, fallback_status: &'static str, work: F)
where
    F: FnOnce() -> AppResult<AppUpdate> + Send + 'static,
{
    thread::spawn(move || {
        let update = match work() {
            Ok(mut update) => {
                if update.status_text.is_none() {
                    update.status_text = Some(fallback_status.to_string());
                }
                update
            }
            Err(error) => AppUpdate {
                now_title: None,
                yt_results: None,
                search_results: None,
                now_playing_url: None,
                status_text: Some(error_status_text(error.as_ref())),
                queue_context: None,
            },
        };

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                if let Some(value) = update.now_title {
                    app.set_now_title(SharedString::from(value));
                    app.set_playback_active(true);
                    app.set_playback_paused(false);
                }
                if let Some(results) = update.yt_results {
                    let playing = update.now_playing_url.clone().unwrap_or_default();
                    let can_repaint = update
                        .queue_context
                        .as_ref()
                        .is_none_or(|context| visible_context_matches(&app, context));
                    if can_repaint {
                        app.set_queue_items(queue_model(&results, &playing));
                        app.set_selected_count(0);
                    }
                }
                if let Some(results) = update.search_results {
                    app.set_search_results(search_model(&results));
                }
                if let Some(value) = update.status_text {
                    // An empty status is the "stale worker" sentinel â€” leave the
                    // current status untouched.
                    if !value.is_empty() {
                        update_helper_repair_prompt(&app, &value);
                        app.set_status_text(SharedString::from(value));
                    }
                }
            }
        });
    });
}

fn current_local_context(queue: &AppQueue) -> QueueContext {
    match &queue.local_kind {
        LocalKind::Library | LocalKind::None => QueueContext::Library,
        LocalKind::Downloads => QueueContext::Downloads,
        LocalKind::Playlist(name) => QueueContext::Playlist(name.clone()),
    }
}

fn visible_context_matches(app: &AppWindow, context: &QueueContext) -> bool {
    match context {
        QueueContext::Stream => app.get_source_mode() == "youtube",
        QueueContext::Library => {
            app.get_source_mode() == "local"
                && app.get_local_tab() == 0
                && app.get_local_list_label() == "LIBRARY"
        }
        QueueContext::Downloads => {
            app.get_source_mode() == "local"
                && app.get_local_tab() == 2
                && app.get_local_list_label() == "DOWNLOADS"
        }
        QueueContext::Playlist(name) => {
            app.get_source_mode() == "local"
                && app.get_local_tab() == 1
                && app.get_local_list_label().as_str() == name
        }
    }
}

// Reserve the next play-request generation. Handlers call this right before
// spawning a background resolver and hand the token to it.
fn next_play_gen(queue: &Arc<Mutex<AppQueue>>) -> u64 {
    queue.lock().map(|mut q| q.bump_play_gen()).unwrap_or(0)
}

// Has a newer play request superseded `gen`? If so, the calling worker must not
// commit audio or repaint.
fn play_gen_is_stale(queue: &Arc<Mutex<AppQueue>>, gen: u64) -> bool {
    queue.lock().map(|q| q.play_gen != gen).unwrap_or(false)
}

fn next_search_gen(queue: &Arc<Mutex<AppQueue>>) -> u64 {
    queue.lock().map(|mut q| q.bump_search_gen()).unwrap_or(0)
}

fn search_gen_is_stale(queue: &Arc<Mutex<AppQueue>>, gen: u64) -> bool {
    queue.lock().map(|q| q.search_gen != gen).unwrap_or(true)
}

fn cancel_search(queue: &Arc<Mutex<AppQueue>>) {
    let _ = next_search_gen(queue);
}

// Sentinel AppUpdate for a stale worker: change nothing, keep the status.
fn stale_update() -> AppUpdate {
    AppUpdate {
        now_title: None,
        yt_results: None,
        search_results: None,
        now_playing_url: None,
        status_text: Some(String::new()),
        queue_context: None,
    }
}

fn set_status(weak: &slint::Weak<AppWindow>, status: &str) {
    if let Some(app) = weak.upgrade() {
        app.set_status_text(SharedString::from(status));
    }
}

fn stop_current_for_new_track(
    weak: &slint::Weak<AppWindow>,
    controller: &Arc<Mutex<PlayerController>>,
    status: &str,
) {
    if let Ok(mut controller) = controller.lock() {
        controller.stop();
    }

    reset_playback_ui(weak, status);
}

fn reset_playback_ui(weak: &slint::Weak<AppWindow>, status: &str) {
    if let Some(app) = weak.upgrade() {
        app.set_progress_percent(0.0);
        app.set_progress_seekable(false);
        app.set_progress_text(SharedString::from("0:00 / 0:00"));
        app.set_playback_active(false);
        app.set_playback_paused(false);
        app.set_status_text(SharedString::from(status));
    }
}

// Live filter for the Local tab. Filters whichever list is open without
// mutating the underlying data: playlist names on the Playlists tab, otherwise
// the visible track list (Downloads or an opened playlist). Empty query restores
// the full list.
fn apply_local_filter(app: &AppWindow, queue: &Arc<Mutex<AppQueue>>, query: &str) {
    let needle = query.trim().to_lowercase();
    let showing_playlist_names =
        app.get_local_tab() == 1 && app.get_local_list_label() == "PLAYLISTS";
    if showing_playlist_names {
        let all = list_playlists().unwrap_or_default();
        let filtered: Vec<Playlist> = if needle.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .collect()
        };
        app.set_playlist_items(playlist_model(&filtered));
    } else {
        let (items, playing) = match queue.lock() {
            Ok(g) => (g.local_items.clone(), g.now_playing_url.clone()),
            Err(_) => return,
        };
        let filtered: Vec<YtResultSlot> = if needle.is_empty() {
            items
        } else {
            items
                .into_iter()
                .filter(|s| s.title.to_lowercase().contains(&needle))
                .collect()
        };
        app.set_queue_items(queue_model(&filtered, &playing));
    }
}

fn play_local_tracks(
    weak: slint::Weak<AppWindow>,
    controller: Arc<Mutex<PlayerController>>,
    queue: Arc<Mutex<AppQueue>>,
    tracks: Vec<PathBuf>,
    gen: u64,
    _detail: &'static str,
) {
    // Apply per-track tune memory before playback so the new VarSpeed wrapper
    // reads the right speed/reverb from the first sample. Local tracks only.
    if let Some(first) = tracks.first() {
        let key = first.to_string_lossy().to_string();
        let saved = load_tune_for(&key);
        if let Some(setting) = saved {
            if let Ok(mut controller) = controller.lock() {
                controller.set_tune(setting.speed, setting.reverb);
            }
        }
        if let Some(app) = weak.upgrade() {
            app.set_tune_can_save(true);
            app.set_tune_saved(saved.is_some());
            if let Some(setting) = saved {
                app.set_tune_speed(setting.speed * 100.0);
                app.set_tune_reverb(setting.reverb * 100.0);
            }
        }
    }
    run_background(weak, "Local playback started.", move || {
        let first_track = tracks.first().ok_or("no local tracks to play")?.clone();
        // Probe duration BEFORE locking the controller â€” this may spawn ffmpeg
        // and take a while; holding the lock across it stalls the UI thread.
        let duration = first_track_duration(&tracks).unwrap_or(None);
        if play_gen_is_stale(&queue, gen) {
            return Ok(stale_update());
        }
        controller
            .lock()
            .map_err(|_| "player controller lock poisoned")?
            .play_files(tracks, duration)
            .map_err(|error| local_playback_error(&first_track, error.as_ref()))?;
        Ok(AppUpdate {
            now_title: Some(display_track_name(&first_track)),
            yt_results: None,
            search_results: None,
            now_playing_url: None,
            status_text: Some("Local playback started.".to_string()),
            queue_context: None,
        })
    });
}

fn local_playback_error(track: &Path, error: &dyn Error) -> Box<dyn Error> {
    if !track.exists() {
        return format!(
            "Track is missing from disk: {}. Rescan Library to clean it up.",
            display_track_name(track)
        )
        .into();
    }

    format!(
        "Could not play {}. The file may be unsupported or damaged: {error}",
        display_track_name(track)
    )
    .into()
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  sidetone scan <music-folder>");
    eprintln!("  sidetone play <audio-file-or-folder>");
    eprintln!("  sidetone yt-search <query>");
    eprintln!("  sidetone yt-resolve <url-or-query>");
    eprintln!("  sidetone yt-play <url-or-query>");
    eprintln!("  sidetone tone");
}

fn scan_command(root: &Path) -> io::Result<()> {
    if !root.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("folder does not exist: {}", root.display()),
        ));
    }

    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a folder: {}", root.display()),
        ));
    }

    let collections = scan_library(root)?;
    let track_count: usize = collections
        .iter()
        .map(|collection| collection.tracks.len())
        .sum();

    println!("Library: {}", root.display());
    println!("Collections: {}", collections.len());
    println!("Tracks: {track_count}");
    println!();

    for collection in collections {
        println!("[{}] {} tracks", collection.name, collection.tracks.len());
        for track in collection.tracks.iter().take(3) {
            println!("  - {}", display_track_name(&track.path));
        }
        if collection.tracks.len() > 3 {
            println!("  ... {} more", collection.tracks.len() - 3);
        }
    }

    Ok(())
}

fn play_command(path: &Path) -> AppResult<()> {
    let tracks = playable_tracks(path)?;

    if tracks.is_empty() {
        return Err(format!("no supported audio files found in {}", path.display()).into());
    }

    let mut stream_handle = rodio::DeviceSinkBuilder::open_default_sink()?;
    stream_handle.log_on_drop(false);
    let player = rodio::Player::connect_new(stream_handle.mixer());

    for track in tracks {
        println!("Queued: {}", track.display());
        let file = File::open(&track)?;
        player.append(rodio::Decoder::try_from(file)?);
    }

    player.sleep_until_end();

    Ok(())
}

fn playable_tracks(path: &Path) -> AppResult<Vec<PathBuf>> {
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()).into());
    }

    if path.is_dir() {
        Ok(tracks_for_folder(path)?)
    } else if is_supported_audio_file(path) {
        Ok(vec![path.to_path_buf()])
    } else {
        Err(format!("unsupported audio file: {}", path.display()).into())
    }
}

fn tone_command() -> AppResult<()> {
    let mut stream_handle = rodio::DeviceSinkBuilder::open_default_sink()?;
    stream_handle.log_on_drop(false);
    let tone = rodio::source::SineWave::new(440.0)
        .amplify(0.18)
        .take_duration(Duration::from_millis(900));

    println!("Playing 440 Hz test tone...");
    stream_handle.mixer().add(tone);
    thread::sleep(Duration::from_millis(1000));

    Ok(())
}

fn yt_search_command(query: &str) -> AppResult<()> {
    let results = yt_search_results(query, 5)?;
    let mut count = 0usize;

    for entry in results {
        count += 1;
        let title = entry.title.as_deref().unwrap_or("Untitled");
        let url = entry
            .webpage_url
            .as_deref()
            .or(entry.url.as_deref())
            .unwrap_or("no url");
        let duration = entry
            .duration
            .map(format_duration)
            .unwrap_or_else(|| "--:--".to_string());

        println!("{count}. {title} [{duration}]");
        println!("   {url}");
    }

    if count == 0 {
        println!("No results.");
    }

    Ok(())
}

fn yt_search_results(query: &str, limit: usize) -> AppResult<Vec<YtEntry>> {
    let target = format!("ytsearch{limit}:{query}");
    let args = vec![
        "--dump-json".to_string(),
        "--flat-playlist".to_string(),
        "--no-warnings".to_string(),
        target,
    ];
    let output = ytdlp_output(&args)?;
    ensure_success("yt-dlp search", &output)?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        results.push(serde_json::from_str(line)?);
    }

    Ok(results)
}

fn yt_play_command(input: &str) -> AppResult<()> {
    let title = yt_play_buffered(input)?;
    println!("Finished: {title}");
    Ok(())
}

// The user's Music folder (and a couple of common fallbacks) for one-click scan.
fn default_music_dir() -> Option<PathBuf> {
    let profile = env::var("USERPROFILE").ok()?;
    for sub in ["Music", "Downloads"] {
        let dir = PathBuf::from(&profile).join(sub);
        if dir.exists() {
            return Some(dir);
        }
    }
    None
}

fn cleanup_temp_dir(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path) {
        eprintln!(
            "Could not remove temp audio folder {}: {error}",
            path.display()
        );
    }
}

fn timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn yt_resolve_command(input: &str) -> AppResult<()> {
    let target = yt_target(input);
    let metadata = yt_metadata(&target)?;
    let title = metadata.title.as_deref().unwrap_or("Untitled");
    let duration = metadata
        .duration
        .map(format_duration)
        .unwrap_or_else(|| "--:--".to_string());
    let page_url = metadata
        .webpage_url
        .as_deref()
        .or(metadata.url.as_deref())
        .unwrap_or("no url");
    let audio_url = yt_print_field(&target, "%(url)s")?;

    println!("Title: {title}");
    println!("Duration: {duration}");
    println!("Page: {page_url}");
    println!("Audio URL: {}", truncate(&audio_url, 120));

    Ok(())
}

fn yt_metadata(target: &str) -> AppResult<YtEntry> {
    let args = vec![
        "--dump-single-json".to_string(),
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        target.to_string(),
    ];
    let output = ytdlp_output(&args)?;
    ensure_success("yt-dlp metadata", &output)?;
    let metadata: YtEntry = serde_json::from_slice(&output.stdout)?;

    if let Some(entries) = &metadata.entries {
        if let Some(first) = entries.iter().find(|entry| entry.title.is_some()) {
            return Ok(first.clone());
        }
    }

    Ok(metadata)
}

fn yt_print_field(target: &str, template: &str) -> AppResult<String> {
    let args = vec![
        "--no-playlist".to_string(),
        "-f".to_string(),
        "bestaudio".to_string(),
        "--no-warnings".to_string(),
        "--print".to_string(),
        template.to_string(),
        target.to_string(),
    ];
    let output = ytdlp_output(&args)?;
    ensure_success("yt-dlp resolve", &output)?;

    let value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    if value.is_empty() {
        return Err(format!("yt-dlp returned an empty value for {template}").into());
    }

    Ok(value)
}

/// Turn search-box input into a yt-dlp target. Only a validated YouTube URL is
/// passed through as a direct target; anything else (plain text, or a non-YouTube
/// URL that slipped through) becomes a YouTube search, so an arbitrary pasted URL
/// is never handed to yt-dlp as a download target.
fn update_helper_repair_prompt(app: &AppWindow, status: &str) {
    if helper_repair_needed_status(status) {
        app.set_streaming_helper_label(SharedString::from(
            "Streaming helper needs repair. Click Repair.",
        ));
        app.set_streaming_helper_action(SharedString::from("Repair"));
    }
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let minutes = total / 60;
    let seconds = total % 60;
    format!("{minutes}:{seconds:02}")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- v7 Phase 4: URL input hardening ------------------------------------

    #[test]
    fn plain_search_text_is_not_a_url_and_stays_a_search() {
        for q in [
            "lofi beats",
            "youtube.com", // bare host, no scheme â†’ search text
            "watch youtube.com videos",
            "the youtu.be song",
            "",
        ] {
            assert!(!is_youtube_url(q), "plain text should not be a URL: {q:?}");
            assert!(!is_playlist_import_url(q));
        }
        // yt_target keeps plain text as a YouTube search.
        assert_eq!(yt_target("lofi beats"), "ytsearch1:lofi beats");
        // A validated YouTube URL is passed straight through.
        assert_eq!(
            yt_target("https://youtu.be/abc123"),
            "https://youtu.be/abc123"
        );
        // A non-YouTube URL is NOT handed to yt-dlp as a target â€” it's searched.
        assert_eq!(
            yt_target("https://evil.com/watch?v=abc"),
            "ytsearch1:https://evil.com/watch?v=abc"
        );
    }

    // --- v7 Phase 4: hotkey registration status -----------------------------

    #[test]
    fn hotkey_status_reports_unavailable_when_manager_fails() {
        let status = hotkey_status_label(false, &[]);
        assert!(status.contains("unavailable"));
    }

    #[test]
    fn hotkey_status_empty_when_all_registered() {
        let slots = [("Play / Pause", true, true), ("Toggle Focus", true, true)];
        assert_eq!(hotkey_status_label(true, &slots), "");
    }

    #[test]
    fn hotkey_status_lists_failed_registrations_only() {
        let slots = [
            ("Play / Pause", true, true),     // ok
            ("Toggle Focus", true, false),    // parsed but not registered â†’ failed
            ("Previous track", false, false), // unparsed combo â†’ not a failure
        ];
        let status = hotkey_status_label(true, &slots);
        assert!(status.contains("Toggle Focus"), "got: {status}");
        assert!(!status.contains("Play / Pause"));
        assert!(!status.contains("Previous track"));
    }
}
