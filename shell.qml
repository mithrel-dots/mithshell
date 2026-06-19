//@ pragma Env QS_NO_RELOAD_POPUP=1
//@ pragma Env QSG_RENDER_LOOP=threaded

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Controls
import QtQuick.Effects
import QtQuick.Layouts
import QtQuick.Shapes

ShellRoot {
    id: shell

    property bool dashboardOpen: false
    property bool dashboardOpening: false
    property real morphProgress: 0
    property var matugen: ({})
    property date now: new Date()
    readonly property color fallbackAccent: "#c7d2ff"
    readonly property color accent: pickColor(["colors.primary", "primary", "m3primary"], fallbackAccent)
    readonly property color accentSoft: Qt.alpha(accent, 0.18)
    readonly property color fg: pickColor(["colors.on_surface", "on_surface", "m3onSurface"], "#f4f4f5")
    readonly property color muted: Qt.alpha(fg, 0.58)
    readonly property color glass: Qt.rgba(0.015, 0.016, 0.018, 0.9)
    readonly property color glassHigh: Qt.rgba(0.03, 0.032, 0.038, 0.96)
    readonly property int splotchOpenDuration: 1650
    readonly property int splotchCloseDuration: 720
    readonly property real splotchOvershoot: 1.45

    Behavior on morphProgress {
        NumberAnimation {
            duration: shell.dashboardOpening ? shell.splotchOpenDuration : shell.splotchCloseDuration
            easing.type: shell.dashboardOpening ? Easing.InOutCubic : Easing.OutCubic
        }
    }

    function pickColor(paths: var, fallback: color): color {
        for (let i = 0; i < paths.length; i++) {
            const value = valueAt(paths[i]);
            if (typeof value === "string" && value.length > 0)
                return value[0] === "#" ? value : `#${value}`;
        }
        return fallback;
    }

    function valueAt(path: string): var {
        const parts = path.split(".");
        let current = matugen;
        for (let i = 0; i < parts.length; i++) {
            if (current === undefined || current === null || !current.hasOwnProperty(parts[i]))
                return undefined;
            current = current[parts[i]];
        }
        return current;
    }

    function clamp(value: real, min: real, max: real): real {
        return Math.max(min, Math.min(max, value));
    }

    function mix(from: real, to: real, amount: real): real {
        return from + (to - from) * amount;
    }

    function smoothstep(edge0: real, edge1: real, value: real): real {
        const x = clamp((value - edge0) / (edge1 - edge0), 0, 1);
        return x * x * (3 - 2 * x);
    }

    function inkKey(idle: real, birth: real, fall: real, reform: real, dashboard: real): real {
        const p = morphProgress;
        const birthT = smoothstep(0.04, 0.28, p);
        const fallT = smoothstep(0.20, 0.43, p);
        const reformT = smoothstep(0.46, 0.68, p);
        const dashT = smoothstep(0.70, 1, p);
        return mix(mix(mix(mix(idle, birth, birthT), fall, fallT), reform, reformT), dashboard, dashT);
    }

    function toggleDashboard(): void {
        setDashboardOpen(!dashboardOpen);
    }

    function setDashboardOpen(open: bool): void {
        if (dashboardOpen === open)
            return;
        dashboardOpening = open;
        dashboardOpen = open;
        morphProgress = open ? 1 : 0;
    }

    Timer {
        interval: 1000
        running: true
        repeat: true
        onTriggered: shell.now = new Date()
    }

    FileView {
        path: `${Quickshell.env("HOME")}/.config/matugen/colors.json`
        watchChanges: true
        onFileChanged: reload()
        onLoaded: {
            try {
                shell.matugen = JSON.parse(text());
            } catch (error) {
                shell.matugen = ({});
            }
        }
        onLoadFailed: shell.matugen = ({})
    }

    PanelWindow {
        id: win

            readonly property bool open: shell.morphProgress >= 0.985
            readonly property bool inInkMotion: shell.morphProgress > 0.02 && shell.morphProgress < 0.985
            readonly property real resolutionScale: Math.max(1.08, Math.min(1.45, width / 1720))
            readonly property int idleWidth: Math.round(186 * resolutionScale)
            readonly property int idleHeight: Math.round(36 * resolutionScale)
            readonly property int idleTopMargin: Math.round(6 * resolutionScale)
            readonly property int birthWidth: Math.round(126 * resolutionScale)
            readonly property int birthHeight: Math.round(104 * resolutionScale)
            readonly property int birthTopMargin: Math.round(18 * resolutionScale)
            readonly property int fallWidth: Math.round(62 * resolutionScale)
            readonly property int fallHeight: Math.round(118 * resolutionScale)
            readonly property int fallTopMargin: Math.round(76 * resolutionScale)
            readonly property int reformWidth: Math.round(174 * resolutionScale)
            readonly property int reformHeight: Math.round(42 * resolutionScale)
            readonly property int reformTopMargin: Math.round(118 * resolutionScale)
            readonly property int dashboardTopMargin: Math.round(118 * resolutionScale)
            readonly property int openWidth: Math.min(Math.round(600 * resolutionScale), width - Math.round(32 * resolutionScale))
            readonly property int openHeight: Math.round(410 * resolutionScale)
            readonly property int shapeWidth: Math.round(shell.inkKey(idleWidth, birthWidth, fallWidth, reformWidth, openWidth))
            readonly property int shapeHeight: Math.round(shell.inkKey(idleHeight, birthHeight, fallHeight, reformHeight, openHeight))
            readonly property int shapeTopMargin: Math.round(shell.inkKey(idleTopMargin, birthTopMargin, fallTopMargin, reformTopMargin, dashboardTopMargin))

            anchors.top: true
            anchors.left: true
            anchors.right: true
            implicitHeight: shapeTopMargin + shapeHeight + Math.round(18 * resolutionScale)
            color: "transparent"
            WlrLayershell.namespace: "mithshell-pill"
            WlrLayershell.layer: WlrLayer.Top
            WlrLayershell.exclusionMode: ExclusionMode.Ignore
            WlrLayershell.keyboardFocus: WlrKeyboardFocus.None

            mask: Region {
                x: splotch.x - 10
                y: splotch.y - 10
                width: splotch.width + 20
                height: splotch.height + 20
            }

            Item {
                id: splotch

                width: win.shapeWidth
                height: win.shapeHeight
                anchors.top: parent.top
                anchors.topMargin: win.shapeTopMargin
                anchors.horizontalCenter: parent.horizontalCenter
                layer.enabled: true
                layer.samples: 8
                layer.effect: MultiEffect {
                    shadowEnabled: true
                    shadowBlur: 1
                    shadowOpacity: win.open ? 0.58 : (win.inInkMotion ? 0.5 : 0.36)
                    shadowVerticalOffset: win.open ? Math.round(22 * win.resolutionScale) : (win.inInkMotion ? Math.round(16 * win.resolutionScale) : Math.round(8 * win.resolutionScale))
                    shadowColor: "#000000"
                }

                Shape {
                    id: body

                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    property real topSpread: shell.inkKey(0.42, 0.31, 0.025, 0.42, 0.43)
                    property real neckSpread: shell.inkKey(0.49, 0.035, 0.08, 0.43, 0.5)
                    property real bodySpread: shell.inkKey(0.49, 0.36, 0.43, 0.49, 0.5)
                    property real topY: shell.inkKey(0.06, 0.02, 0, 0.04, 0)
                    property real neckY: shell.inkKey(0.3, 0.35, 0.28, 0.26, 0.2)
                    property real bodyY: shell.inkKey(0.62, 0.76, 0.76, 0.64, 0.72)
                    property real bottomY: shell.inkKey(0.92, 0.98, 1, 0.92, 1)

                    ShapePath {
                        fillColor: win.open ? shell.glassHigh : shell.glass
                        strokeColor: Qt.alpha(shell.accent, win.open ? 0.42 : 0.28)
                        strokeWidth: 1
                        capStyle: ShapePath.RoundCap
                        joinStyle: ShapePath.RoundJoin

                        Behavior on fillColor { ColorAnimation { duration: 220 } }
                        Behavior on strokeColor { ColorAnimation { duration: 220 } }

                        startX: body.width * 0.5
                        startY: body.height * body.topY

                        PathCubic {
                            control1X: body.width * (0.5 + body.topSpread * 0.32)
                            control1Y: body.height * body.topY
                            control2X: body.width * (0.5 + body.topSpread * 0.82)
                            control2Y: body.height * body.topY
                            x: body.width * (0.5 + body.topSpread)
                            y: body.height * body.topY
                        }

                        PathCubic {
                            control1X: body.width * (0.5 + Math.max(body.topSpread, body.neckSpread))
                            control1Y: body.height * (body.topY + (body.neckY - body.topY) * 0.28)
                            control2X: body.width * (0.5 + body.neckSpread)
                            control2Y: body.height * (body.topY + (body.neckY - body.topY) * 0.82)
                            x: body.width * (0.5 + body.neckSpread)
                            y: body.height * body.neckY
                        }

                        PathCubic {
                            control1X: body.width * (0.5 + body.neckSpread)
                            control1Y: body.height * (body.neckY + (body.bodyY - body.neckY) * 0.36)
                            control2X: body.width * (0.5 + body.bodySpread)
                            control2Y: body.height * (body.neckY + (body.bodyY - body.neckY) * 0.82)
                            x: body.width * (0.5 + body.bodySpread)
                            y: body.height * body.bodyY
                        }

                        PathCubic {
                            control1X: body.width * (0.5 + body.bodySpread)
                            control1Y: body.height * (body.bodyY + (body.bottomY - body.bodyY) * 0.72)
                            control2X: body.width * 0.68
                            control2Y: body.height * body.bottomY
                            x: body.width * 0.5
                            y: body.height * body.bottomY
                        }

                        PathCubic {
                            control1X: body.width * 0.32
                            control1Y: body.height * body.bottomY
                            control2X: body.width * (0.5 - body.bodySpread)
                            control2Y: body.height * (body.bodyY + (body.bottomY - body.bodyY) * 0.72)
                            x: body.width * (0.5 - body.bodySpread)
                            y: body.height * body.bodyY
                        }

                        PathCubic {
                            control1X: body.width * (0.5 - body.bodySpread)
                            control1Y: body.height * (body.neckY + (body.bodyY - body.neckY) * 0.82)
                            control2X: body.width * (0.5 - body.neckSpread)
                            control2Y: body.height * (body.neckY + (body.bodyY - body.neckY) * 0.36)
                            x: body.width * (0.5 - body.neckSpread)
                            y: body.height * body.neckY
                        }

                        PathCubic {
                            control1X: body.width * (0.5 - body.neckSpread)
                            control1Y: body.height * (body.topY + (body.neckY - body.topY) * 0.82)
                            control2X: body.width * (0.5 - Math.max(body.topSpread, body.neckSpread))
                            control2Y: body.height * (body.topY + (body.neckY - body.topY) * 0.28)
                            x: body.width * (0.5 - body.topSpread)
                            y: body.height * body.topY
                        }

                        PathCubic {
                            control1X: body.width * (0.5 - body.topSpread * 0.82)
                            control1Y: body.height * body.topY
                            control2X: body.width * (0.5 - body.topSpread * 0.32)
                            control2Y: body.height * body.topY
                            x: body.width * 0.5
                            y: body.height * body.topY
                        }
                    }
                }

                RowLayout {
                    id: pillContent

                    z: 2
                    anchors.centerIn: parent
                    anchors.horizontalCenterOffset: 0
                    width: Math.min(parent.width - Math.round(28 * win.resolutionScale), implicitWidth)
                    opacity: 1 - shell.smoothstep(0.04, 0.22, shell.morphProgress)
                    spacing: Math.round(9 * win.resolutionScale)

                    Behavior on opacity { NumberAnimation { duration: 260 } }

                    Rectangle {
                        width: Math.round(7 * win.resolutionScale)
                        height: width
                        radius: 4
                        color: shell.accent
                        layer.enabled: true
                        layer.effect: MultiEffect {
                            shadowEnabled: true
                            shadowBlur: 1
                            shadowOpacity: 0.72
                            shadowColor: shell.accent
                        }
                    }

                    Text {
                        text: Qt.formatDateTime(shell.now, "hh:mm")
                        color: shell.fg
                        font.family: "Rubik, Inter, sans-serif"
                        font.pixelSize: Math.round(13 * win.resolutionScale)
                        font.weight: Font.DemiBold
                    }

                    Text {
                        text: "mithshell"
                        color: shell.muted
                        font.family: "Rubik, Inter, sans-serif"
                        font.pixelSize: Math.round(11 * win.resolutionScale)
                    }
                }

                ColumnLayout {
                    id: dashboard

                    z: 2
                    anchors.fill: parent
                    anchors.margins: Math.round(28 * win.resolutionScale)
                    anchors.topMargin: Math.round(32 * win.resolutionScale)
                    spacing: Math.round(16 * win.resolutionScale)
                    opacity: shell.smoothstep(0.86, 1, shell.morphProgress)
                    scale: 0.965 + 0.035 * shell.smoothstep(0.78, 1, shell.morphProgress)
                    visible: shell.morphProgress > 0.78 || opacity > 0

                    Behavior on opacity {
                        NumberAnimation {
                            duration: shell.dashboardOpening ? 420 : 140
                            easing.type: Easing.OutCubic
                        }
                    }
                    Behavior on scale {
                        NumberAnimation {
                            duration: 420
                            easing.type: Easing.OutCubic
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: Math.round(14 * win.resolutionScale)

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: Math.round(3 * win.resolutionScale)

                            Text {
                                text: "Dashboard"
                                color: shell.fg
                                font.family: "Rubik, Inter, sans-serif"
                                font.pixelSize: Math.round(22 * win.resolutionScale)
                                font.weight: Font.DemiBold
                            }

                            Text {
                                text: "minimal shell surface"
                                color: shell.muted
                                font.family: "Rubik, Inter, sans-serif"
                                font.pixelSize: Math.round(12 * win.resolutionScale)
                            }
                        }

                        GlassButton {
                            label: "close"
                            uiScale: win.resolutionScale
                            accent: shell.accent
                            fg: shell.fg
                            muted: shell.muted
                            onClicked: shell.setDashboardOpen(false)
                        }
                    }

                    GridLayout {
                        Layout.fillWidth: true
                        columns: 2
                        rowSpacing: Math.round(12 * win.resolutionScale)
                        columnSpacing: Math.round(12 * win.resolutionScale)

                        InfoCard { title: "time"; value: Qt.formatDateTime(shell.now, "hh:mm"); uiScale: win.resolutionScale; accent: shell.accent; fg: shell.fg; muted: shell.muted }
                        InfoCard { title: "date"; value: Qt.formatDateTime(shell.now, "ddd d MMM"); uiScale: win.resolutionScale; accent: shell.accent; fg: shell.fg; muted: shell.muted }
                        InfoCard { title: "accent"; value: "matugen ready"; uiScale: win.resolutionScale; accent: shell.accent; fg: shell.fg; muted: shell.muted }
                        InfoCard { title: "mode"; value: "black glass"; uiScale: win.resolutionScale; accent: shell.accent; fg: shell.fg; muted: shell.muted }
                    }

                    Rectangle {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        radius: Math.round(24 * win.resolutionScale)
                        color: Qt.rgba(1, 1, 1, 0.09)
                        border.width: 1
                        border.color: Qt.rgba(1, 1, 1, 0.12)

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: Math.round(18 * win.resolutionScale)
                            spacing: Math.round(10 * win.resolutionScale)

                            Text {
                                text: "MVP controls"
                                color: shell.fg
                                font.family: "Rubik, Inter, sans-serif"
                                font.pixelSize: Math.round(14 * win.resolutionScale)
                                font.weight: Font.DemiBold
                            }

                            Text {
                                Layout.fillWidth: true
                                text: "Click the top pill or call IPC to toggle this splotch panel. Future panels can reuse the same morph state and matugen accent tokens."
                                wrapMode: Text.WordWrap
                                color: shell.muted
                                font.family: "Rubik, Inter, sans-serif"
                                font.pixelSize: Math.round(12 * win.resolutionScale)
                                lineHeight: 1.18
                            }
                        }
                    }
                }

                MouseArea {
                    z: 1
                    anchors.fill: parent
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: mouse => {
                        if (mouse.button === Qt.LeftButton)
                            shell.toggleDashboard();
                        else
                            shell.setDashboardOpen(false);
                    }
                }
            }
    }

    IpcHandler {
        target: "mithshell"

        function toggleDashboard(): void { shell.toggleDashboard(); }
        function openDashboard(): void { shell.setDashboardOpen(true); }
        function closeDashboard(): void { shell.setDashboardOpen(false); }
    }

    component InfoCard: Rectangle {
        required property string title
        required property string value
        required property real uiScale
        required property color accent
        required property color fg
        required property color muted

        Layout.fillWidth: true
        Layout.preferredHeight: Math.round(82 * uiScale)
        radius: Math.round(22 * uiScale)
        color: Qt.rgba(1, 1, 1, 0.1)
        border.width: 1
        border.color: Qt.rgba(1, 1, 1, 0.13)

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: Math.round(14 * uiScale)
            spacing: Math.round(4 * uiScale)

            Text {
                text: title
                color: muted
                font.family: "Rubik, Inter, sans-serif"
                font.pixelSize: Math.round(11 * uiScale)
                font.capitalization: Font.AllLowercase
            }

            Text {
                text: value
                color: fg
                elide: Text.ElideRight
                Layout.fillWidth: true
                font.family: "Rubik, Inter, sans-serif"
                font.pixelSize: Math.round(15 * uiScale)
                font.weight: Font.DemiBold
            }
        }

        Rectangle {
            width: Math.round(34 * uiScale)
            height: Math.max(3, Math.round(3 * uiScale))
            radius: 3
            anchors.left: parent.left
            anchors.bottom: parent.bottom
            anchors.leftMargin: Math.round(14 * uiScale)
            anchors.bottomMargin: Math.round(12 * uiScale)
            color: accent
            opacity: 0.8
        }
    }

    component GlassButton: MouseArea {
        required property string label
        required property real uiScale
        required property color accent
        required property color fg
        required property color muted

        implicitWidth: buttonLabel.implicitWidth + Math.round(28 * uiScale)
        implicitHeight: Math.round(34 * uiScale)
        cursorShape: Qt.PointingHandCursor

        Rectangle {
            anchors.fill: parent
            radius: height / 2
            color: parent.containsMouse ? Qt.alpha(accent, 0.22) : Qt.rgba(1, 1, 1, 0.1)
            border.width: 1
            border.color: parent.containsMouse ? Qt.alpha(accent, 0.48) : Qt.rgba(1, 1, 1, 0.14)

            Behavior on color { ColorAnimation { duration: 160 } }
            Behavior on border.color { ColorAnimation { duration: 160 } }
        }

        Text {
            id: buttonLabel

            anchors.centerIn: parent
            text: label
            color: parent.containsMouse ? fg : muted
            font.family: "Rubik, Inter, sans-serif"
            font.pixelSize: Math.round(12 * parent.uiScale)
            font.weight: Font.DemiBold

            Behavior on color { ColorAnimation { duration: 160 } }
        }
    }
}
