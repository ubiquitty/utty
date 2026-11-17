// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
#include "core.h"
#include <Core/Session/ControlPaths.h>
#include <Core/Session/RemoteBootstrap.h>
#include <Core/Session/SessionDirectoryCore.h>
#include <UbiquittyKit/UbiquittyKit.h>
#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <climits>
#include <cmath>
#include <cstring>
#include <fcntl.h>
#include <map>
#include <mutex>
#include <optional>
#include <poll.h>
#include <signal.h>
#include <string>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#if defined(__APPLE__)
#include <libproc.h>
#endif
#include <vector>

UBIQUITTY_KIT_REQUIRE(2, 1, 0);
static_assert(Ubiquitty::coreVersion >= 10200);

namespace {
using namespace Ubiquitty;
using Clock = std::chrono::steady_clock;
// Core installs its PAL context once per process and aborts on a second
// install; a listing followed by a workspace both need it.
void installPAL()
{
    static std::once_flag installed;
    std::call_once(installed, [] { PAL::installContext({ .monotonicClock = PAL::monotonicTicks }); });
}
std::string encodeUTF8(uint32_t point)
{
    std::string value;
    if (point <= 0x7f)
        value += static_cast<char>(point);
    else if (point <= 0x7ff) {
        value += static_cast<char>(0xc0 | (point >> 6));
        value += static_cast<char>(0x80 | (point & 63));
    } else if (point <= 0xffff) {
        value += static_cast<char>(0xe0 | (point >> 12));
        value += static_cast<char>(0x80 | ((point >> 6) & 63));
        value += static_cast<char>(0x80 | (point & 63));
    } else {
        value += static_cast<char>(0xf0 | (point >> 18));
        value += static_cast<char>(0x80 | ((point >> 12) & 63));
        value += static_cast<char>(0x80 | ((point >> 6) & 63));
        value += static_cast<char>(0x80 | (point & 63));
    }
    return value;
}
class Dispatcher final : public Session::Dispatcher {
public:
    Dispatcher()
    {
        if (::pipe(m_wakeup.data()) < 0)
            return;
        for (auto descriptor : m_wakeup) {
            if (::fcntl(descriptor, F_SETFL, O_NONBLOCK) < 0 || ::fcntl(descriptor, F_SETFD, FD_CLOEXEC) < 0) {
                closeWakeup();
                break;
            }
        }
    }
    ~Dispatcher() override { closeWakeup(); }
    bool isValid() const { return m_wakeup[0] >= 0; }
    int descriptor() const { return m_wakeup[0]; }
    int timeoutMilliseconds() const
    {
        std::lock_guard lock(m_mutex);
        if (m_work.empty())
            return -1;
        const auto delay = m_work.begin()->first.first - Clock::now();
        if (delay <= Clock::duration::zero())
            return 0;
        return static_cast<int>(std::min<std::int64_t>(INT_MAX, std::chrono::ceil<std::chrono::milliseconds>(delay).count()));
    }
    void post(Session::PostedWork work) override { postDelayed(0, std::move(work)); }
    void postDelayed(double seconds, Session::PostedWork work) override
    {
        bool shouldWake = false;
        {
            std::lock_guard lock(m_mutex);
            const auto deadline = Clock::now() + std::chrono::duration_cast<Clock::duration>(std::chrono::duration<double>(seconds));
            shouldWake = m_work.empty() || deadline < m_work.begin()->first.first;
            m_work.emplace(std::make_pair(deadline, m_sequence++), std::move(work));
        }
        if (!shouldWake)
            return;
        const char byte = 0;
        // A full pipe already wakes the owner. Retry an interrupted write so a
        // task posted between the owner's queue check and poll cannot be lost.
        while (::write(m_wakeup[1], &byte, 1) < 0 && errno == EINTR) {
        }
    }
    void drain()
    {
        std::array<char, 256> notifications;
        while (true) {
            const auto count = ::read(m_wakeup[0], notifications.data(), notifications.size());
            if (count > 0 || (count < 0 && errno == EINTR))
                continue;
            break;
        }
        {
            std::lock_guard lock(m_mutex);
            const auto now = Clock::now();
            while (!m_work.empty() && m_work.begin()->first.first <= now) {
                auto node = m_work.extract(m_work.begin());
                m_ready.push_back(std::move(node.mapped()));
            }
        }
        // A batch contains the work ready on entry; callbacks posted by this
        // batch run on the next turn, leaving input and rendering a turn too.
        for (auto& work : m_ready)
            std::move(work)();
        m_ready.clear();
    }

private:
    void closeWakeup()
    {
        for (auto descriptor : m_wakeup)
            if (descriptor >= 0)
                ::close(descriptor);
        m_wakeup = { -1, -1 };
    }
    std::array<int, 2> m_wakeup { -1, -1 };
    mutable std::mutex m_mutex;
    std::map<std::pair<Clock::time_point, std::uint64_t>, Session::PostedWork> m_work;
    std::vector<Session::PostedWork> m_ready;
    std::uint64_t m_sequence = 0;
};

std::string quoted(std::string_view text)
{
    static constexpr char hex[] = "0123456789abcdef";
    std::string result = "\"";
    for (unsigned char byte : text) {
        if (byte == '"' || byte == '\\') {
            result += '\\';
            result += static_cast<char>(byte);
        } else if (byte < 32) {
            result += "\\u00";
            result += hex[byte >> 4];
            result += hex[byte & 15];
        } else
            result += static_cast<char>(byte);
    }
    return result + '"';
}
std::string paneTitle(const Session::TreePane& pane)
{
    if (!pane.label.empty())
        return pane.label;
    if (!pane.facts.programTitle.empty())
        return pane.facts.programTitle;
    if (!pane.facts.foreground.name.empty())
        return pane.facts.foreground.name;
    return pane.program.empty() ? "shell" : pane.program;
}
std::string tabTitle(const Session::TreeTab& tab)
{
    if (!tab.name.empty())
        return tab.name;
    if (!tab.label.empty())
        return tab.label;
    return tab.panes.empty() ? "tab" : paneTitle(tab.panes[std::min(tab.hostFocusedPane, tab.panes.size() - 1)]);
}
std::string topology(const Protocol::Sync::TopologyNode& node, int depth = 0)
{
    std::string result = "{\"horizontal\":" + std::string(node.axis == Protocol::Sync::SplitAxis::Horizontal ? "true" : "false")
        + ",\"ratio\":" + std::to_string(std::isfinite(node.ratio) ? node.ratio : 0.5) + ",\"children\":[";
    if (depth < 64)
        for (std::size_t index = 0; index < node.children.size(); ++index) {
            if (index)
                result += ',';
            result += topology(node.children[index], depth + 1);
        }
    return result + "]}";
};

struct DirectoryAnswer final : Session::SessionDirectoryObserver {
    std::optional<Session::SessionTree> tree;
    std::string error;
    bool done = false;
    void onSessionTree(const Session::SessionTree& value) override { tree = value; }
    void onSessionList(const std::vector<Session::SessionRow>&) override { done = true; }
    void onDirectoryFailed(const Session::DirectoryFailureReport& report) override
    {
        error = Session::listingFailureNotice(report, Session::NoticePlacement::Standalone);
        done = true;
    }
};
std::string directoryJSON(const Session::SessionTree& tree, std::string_view daemonSocket, std::uint64_t daemonInstance)
{
    std::string json = "{\"socket\":" + quoted(daemonSocket) + ",\"daemon_instance\":" + std::to_string(daemonInstance) + ",\"now_ms\":" + std::to_string(tree.hostNowUnixMilliseconds) + ",\"workspaces\":[";
    for (std::size_t index = 0; index < tree.workspaces.size() && index < 4096; ++index) {
        const auto& workspace = tree.workspaces[index];
        std::size_t tabs = 0, panes = 0;
        for (const auto& window : workspace.windows) {
            tabs += window.tabs.size();
            for (const auto& tab : window.tabs)
                panes += tab.panes.size();
        }
        if (index)
            json += ',';
        json += "{\"id\":" + std::to_string(workspace.id) + ",\"name\":" + quoted(workspace.name)
            + ",\"title\":" + quoted(workspace.label.empty() ? workspace.defaultName() : workspace.label)
            + ",\"restorable\":" + (workspace.isRestorable() ? "true" : "false")
            + ",\"windows\":" + std::to_string(workspace.windows.size()) + ",\"tabs\":" + std::to_string(tabs) + ",\"panes\":" + std::to_string(panes)
            + ",\"people\":" + std::to_string(workspace.attendance.people) + ",\"agents\":" + std::to_string(workspace.attendance.agents)
            + ",\"last_input_ms\":" + std::to_string(workspace.activity.lastInputUnixMilliseconds)
            + ",\"last_output_ms\":" + std::to_string(workspace.activity.lastOutputUnixMilliseconds) + "}";
    }
    json += "],\"instances\":[";
    std::size_t listed = 0;
    for (std::size_t index = 0; index < tree.hostInstances.size() && index < Session::maxDiscoveredHostInstances; ++index) {
        const auto& instance = tree.hostInstances[index];
        // The host lists every live daemon, the one answering among them.
        if (instance.socketName == daemonSocket)
            continue;
        if (listed++)
            json += ',';
        json += "{\"socket\":" + quoted(instance.socketName) + ",\"version\":" + quoted(instance.appVersion)
            + ",\"started_s\":" + std::to_string(instance.startedUnixSeconds) + "}";
    }
    return json + "]}";
}
char* copyText(const std::string& text)
{
    auto result = std::make_unique<char[]>(text.size() + 1);
    std::memcpy(result.get(), text.c_str(), text.size() + 1);
    return result.release();
}
}

struct UBQWorkspace final : UbiquittyKit::Workspace::Observer {
    UbiquittyKit::HostConnectionOptions options;
    std::unique_ptr<UbiquittyKit::Workspace> workspace;
    std::vector<std::string> text;
    std::vector<UBQCell> cells;
    std::vector<UBQQuad> quads;
    std::vector<Image::ImageQuad> imageQuads;
    std::map<uint32_t, int> viewOffsets;
    int cellWidth = 8, cellHeight = 16;
    std::string json, error, notice;
    std::uint32_t reveal = 0;
    std::uint64_t revealRevision = 0;
    int columns, rows;
    bool dirty = true;

    UBQWorkspace(const char* host, const char* ssh, const char* socket, int columns, int rows, int)
        : options { host, ssh, socket }
        , columns(columns)
        , rows(rows)
    {
        installPAL();
    }
    void onChanged() override { dirty = true; }
    void onIntentResult(const Session::WorkspaceIntentResult& result) override
    {
        notice = result.wasPerformed ? "" : "Host refused the workspace change";
        dirty = true;
    }
    void onDirectoryError(std::string_view value) override
    {
        notice = value;
        dirty = true;
    }
    // A daemon instance pins the target to the daemon that listed it, which
    // is what lets a workspace nothing runs in be restored by id.
    void attach(std::uint32_t session, std::uint32_t identity, std::string name = {}, std::uint64_t daemonInstance = 0)
    {
        if (workspace && !workspace->socketName().empty())
            options.socketName = workspace->socketName();
        workspace.reset();
        auto factory = UbiquittyKit::makeHostChannelFactory(options);
        if (!factory) {
            error = factory.error();
            return;
        }
        auto dispatcher = std::make_unique<Dispatcher>();
        if (!dispatcher->isValid()) {
            error = "Could not create Core event wakeup pipe";
            return;
        }
        workspace = std::make_unique<UbiquittyKit::Workspace>(std::move(dispatcher), std::move(*factory), *this);
        workspace->start({ session, identity, std::move(name), daemonInstance }, columns, std::max(1, rows - 1));
        reveal = session;
        revealRevision = 0;
        error.clear();
        notice.clear();
        dirty = true;
    }
    const Session::TreeWindow* window() const
    {
        auto* tree = workspace ? workspace->tree() : nullptr;
        return !tree || tree->windows.empty() ? nullptr : &tree->windows[std::min(tree->hostActiveWindow, tree->windows.size() - 1)];
    }
    const Session::TreeTab* tab() const
    {
        auto* value = window();
        return !value || value->tabs.empty() ? nullptr : &value->tabs[std::min(value->hostActiveTab, value->tabs.size() - 1)];
    }
    const Session::TreePane* focused() const
    {
        auto* value = tab();
        if (!value || value->panes.empty())
            return nullptr;
        if (auto zoom = window()->hostZoomedPane; zoom && zoom->tab < window()->tabs.size() && zoom->pane < window()->tabs[zoom->tab].panes.size())
            return &window()->tabs[zoom->tab].panes[zoom->pane];
        return &value->panes[std::min(value->hostFocusedPane, value->panes.size() - 1)];
    }
    std::uint32_t session() const { return focused() ? focused()->sessionID : workspace ? workspace->initialSessionID()
                                                                                        : 0; }
    void send(Session::WorkspaceIntent request)
    {
        notice = workspace && workspace->sendIntent(session(), std::move(request)) ? "" : "Workspace change is unavailable; wait for the host";
        dirty = true;
    }
    Session::WorkspaceIntent intent(Session::WorkspaceIntentKind kind) const
    {
        Session::WorkspaceIntent result;
        result.verb = kind;
        if (window())
            result.surfaceID = std::to_underlying(window()->id);
        if (tab())
            result.tabID = std::to_underlying(tab()->id);
        if (focused())
            result.paneID = std::to_underlying(focused()->id);
        return result;
    }
    bool tick()
    {
        if (!workspace || !error.empty())
            return false;
        static_cast<Dispatcher&>(workspace->dispatcher()).drain();
        if (dirty) {
            workspace->synchronize();
            if (auto* tree = workspace->tree(); reveal && tree && tree->revision != revealRevision) {
                for (const auto& surface : tree->windows)
                    for (const auto& value : surface.tabs)
                        for (const auto& pane : value.panes) {
                            if (pane.sessionID != reveal)
                                continue;
                            auto request = intent(Session::WorkspaceIntentKind::FocusPane);
                            request.surfaceID = std::to_underlying(surface.id);
                            request.tabID = std::to_underlying(value.id);
                            request.paneID = std::to_underlying(pane.id);
                            if (!surface.isHostActive)
                                request.verb = Session::WorkspaceIntentKind::SetActiveSurface;
                            else if (!value.isHostActive)
                                request.verb = Session::WorkspaceIntentKind::SetActiveTab;
                            else if (pane.isHostFocused) {
                                reveal = 0;
                                continue;
                            }
                            if (workspace->sendIntent(reveal, request))
                                revealRevision = tree->revision;
                        }
            }
        }
        if (!workspace->error().empty())
            error = workspace->error();
        if (workspace->hasEnded()) {
            error.clear();
            return false;
        }
        if (workspace->state() == Session::State::Failed) {
            if (error.empty())
                error = "Workspace connection failed";
            return false;
        }
        return error.empty();
    }
    const char* snapshot()
    {
        json = "{\"session\":" + std::to_string(session()) + ",\"socket\":" + quoted(workspace ? workspace->socketName() : "") + ",\"error\":" + quoted(error) + ",\"notice\":" + quoted(notice);
        json += ",\"ended\":" + std::string(workspace && workspace->hasEnded() ? "true" : "false");
        std::string state = !workspace || workspace->state() != Session::State::Connected || reveal ? "connecting" : "";
        if (workspace && workspace->state() == Session::State::Reconnecting)
            state = "reconnecting";
        json += ",\"state\":" + quoted(state) + ",\"name\":" + quoted(workspace && workspace->tree() ? workspace->tree()->name : "") + ",\"tabs\":[";
        if (window()) {
            for (std::size_t index = 0; index < window()->tabs.size(); ++index) {
                const auto& value = window()->tabs[index];
                if (index)
                    json += ',';
                json += "{\"id\":" + std::to_string(std::to_underlying(value.id)) + ",\"focused_pane\":" + std::to_string(value.panes.empty() ? 0 : std::to_underlying(value.panes[std::min(value.hostFocusedPane, value.panes.size() - 1)].id)) + ",\"pane_ids\":[";
                for (std::size_t paneIndex = 0; paneIndex < value.panes.size(); ++paneIndex) {
                    if (paneIndex)
                        json += ',';
                    json += std::to_string(std::to_underlying(value.panes[paneIndex].id));
                }
                json += "],\"title\":" + quoted(tabTitle(value)) + ",\"kind\":" + std::to_string(std::to_underlying(value.status.kind)) + ",\"active\":" + (value.isHostActive ? "true" : "false") + "}";
            }
        }
        json += "],\"panes\":[";
        if (tab())
            for (std::size_t index = 0; index < tab()->panes.size(); ++index) {
                const auto& pane = tab()->panes[index];
                if (index)
                    json += ',';
                json += "{\"id\":" + std::to_string(std::to_underlying(pane.id)) + ",\"session\":" + std::to_string(pane.sessionID)
                    + ",\"focused\":" + (pane.sessionID == session() ? "true" : "false") + ",\"title\":" + quoted(paneTitle(pane))
                    + ",\"kind\":" + std::to_string(std::to_underlying(pane.status.kind)) + "}";
            }
        json += "],\"directory\":[";
        // One row per workspace, window, tab and pane. `session` and
        // `workspace` are the attach target: a live row opens its pane's
        // session, and every row of a restorable workspace, whose panes run
        // nothing, opens the workspace itself.
        std::size_t count = 0;
        auto append = [&](int kind, std::uint32_t sessionID, const Session::TreeWorkspace& owner, std::string_view title, std::string_view detail, int status) {
            if (count >= 4096)
                return;
            if (count++)
                json += ',';
            const bool restorable = owner.isRestorable();
            std::size_t panes = 0;
            if (kind == 0)
                for (const auto& surface : owner.windows)
                    for (const auto& value : surface.tabs)
                        panes += value.panes.size();
            json += "{\"kind\":" + std::to_string(kind) + ",\"session\":" + std::to_string(restorable || kind == 0 ? 0 : sessionID)
                + ",\"workspace\":" + std::to_string(restorable || kind == 0 ? owner.id : 0) + ",\"owner\":" + std::to_string(owner.id)
                + ",\"name\":" + quoted(owner.name) + ",\"title\":" + quoted(title) + ",\"detail\":" + quoted(detail)
                + ",\"status\":" + std::to_string(status) + ",\"restorable\":" + (restorable ? "true" : "false")
                + ",\"people\":" + std::to_string(owner.attendance.people) + ",\"panes\":" + std::to_string(panes)
                + ",\"active_ms\":" + std::to_string(std::max(owner.activity.lastInputUnixMilliseconds, owner.activity.lastOutputUnixMilliseconds)) + "}";
        };
        if (workspace)
            for (const auto& value : workspace->directory().workspaces) {
                append(0, 0, value, value.label.empty() ? value.defaultName() : value.label, {}, std::to_underlying(value.status.kind));
                for (const auto& surface : value.windows) {
                    if (surface.tabs.empty())
                        continue;
                    const auto& front = surface.tabs[std::min(surface.hostActiveTab, surface.tabs.size() - 1)];
                    if (front.panes.empty())
                        continue;
                    append(1, front.panes[std::min(front.hostFocusedPane, front.panes.size() - 1)].sessionID, value,
                        surface.label.empty() ? "Window " + std::to_string(&surface - value.windows.data() + 1) : surface.label, {}, std::to_underlying(surface.status.kind));
                    for (const auto& candidate : surface.tabs) {
                        if (candidate.panes.empty())
                            continue;
                        append(2, candidate.panes[std::min(candidate.hostFocusedPane, candidate.panes.size() - 1)].sessionID, value,
                            std::to_string(&candidate - surface.tabs.data() + 1) + ":" + tabTitle(candidate), {}, std::to_underlying(candidate.status.kind));
                        for (const auto& pane : candidate.panes)
                            append(3, pane.sessionID, value, paneTitle(pane), pane.directory(), std::to_underlying(pane.status.kind));
                    }
                }
            }
        json += "],\"workspace_id\":" + std::to_string(workspace && workspace->tree() ? workspace->tree()->id : 0)
            + ",\"now_ms\":" + std::to_string(workspace ? workspace->directory().hostNowUnixMilliseconds : 0);
        json += ",\"surface_id\":" + std::to_string(window() ? std::to_underlying(window()->id) : 0)
            + ",\"tab_id\":" + std::to_string(tab() ? std::to_underlying(tab()->id) : 0)
            + ",\"pane_id\":" + std::to_string(focused() ? std::to_underlying(focused()->id) : 0)
            + ",\"zoomed\":" + (window() && window()->hostZoomedPane ? "true" : "false")
            + ",\"split\":" + (tab() ? topology(tab()->split) : "{}") + ",\"surfaces\":[";
        if (workspace && workspace->tree())
            for (std::size_t index = 0; index < workspace->tree()->windows.size(); ++index) {
                if (index)
                    json += ',';
                json += std::to_string(std::to_underlying(workspace->tree()->windows[index].id));
            }
        json += "],\"listing\":" + std::string(workspace && workspace->directoryIsLive() ? "false" : "true") + ",\"kind\":" + std::to_string(window() ? std::to_underlying(window()->status.kind) : 0);
        json += ",\"detail\":\"\",\"host\":" + quoted(focused() ? focused()->facts.hostName : "") + ",\"cwd\":" + quoted(focused() ? focused()->directory() : "");
        const Session::TreePane* source = focused();
        if (window())
            for (const auto& value : window()->tabs)
                for (const auto& pane : value.panes)
                    if (pane.status.kind == window()->status.kind && (!source || source->status.kind != window()->status.kind))
                        source = &pane;
        auto* record = source ? source->records.store.primary() : nullptr;
        json += ",\"message\":" + quoted(record ? record->message.view() : "") + ",\"progress\":" + std::to_string(record ? record->progress : -1);
        std::string block;
        if (window())
            switch (window()->status.blockKind) {
            case OSC::ProgramBlockKind::Auth:
                block = "auth";
                break;
            case OSC::ProgramBlockKind::Permission:
                block = "permission";
                break;
            case OSC::ProgramBlockKind::Question:
                block = "question";
                break;
            default:
                break;
            }
        json += ",\"block\":" + quoted(block) + ",\"app\":" + quoted(record ? source->records.store.effectiveApp(*record) : "utty");
        // Only a program-supplied title: the pane title already reaches the outer terminal through the window title.
        json += ",\"status_title\":" + quoted(record ? record->title.view() : "");
        json += ",\"progress_state\":" + std::to_string(window() ? window()->status.progress.state : 0) + ",\"progress_value\":" + std::to_string(window() ? window()->status.progress.value : 0) + "}";
        dirty = false;
        return json.c_str();
    }
};

extern "C" {
UBQWorkspace* ubq_open(const char* host, const char* ssh, const char* socket, uint32_t session, uint32_t workspace, uint64_t daemon_instance, const char* name, int columns, int rows, int graphics)
{
    auto owner = std::make_unique<UBQWorkspace>(host, ssh, socket, columns, rows, graphics);
    owner->attach(session, workspace, name, daemon_instance);
    return owner.release();
}
void ubq_close(UBQWorkspace* owner) { delete owner; }
int ubq_tick(UBQWorkspace* owner) { return owner->tick(); }
int ubq_dirty(const UBQWorkspace* owner) { return owner->dirty; }
int ubq_wait_descriptor(const UBQWorkspace* owner)
{
    return owner->workspace ? static_cast<Dispatcher&>(owner->workspace->dispatcher()).descriptor() : -1;
}
int ubq_wait_timeout(const UBQWorkspace* owner)
{
    return owner->workspace ? static_cast<Dispatcher&>(owner->workspace->dispatcher()).timeoutMilliseconds() : -1;
}
const char* ubq_snapshot(UBQWorkspace* owner) { return owner->snapshot(); }
void ubq_resize(UBQWorkspace* owner, int columns, int rows)
{
    owner->columns = columns;
    owner->rows = rows;
    owner->dirty = true;
}
void ubq_input(UBQWorkspace* owner, const uint8_t* bytes, size_t length)
{
    if (owner->workspace && !owner->reveal) {
        auto* terminal = owner->workspace->terminal(owner->session());
        if (terminal && Modes::shouldSnapToTail(terminal, Modes::SnapTrigger::Keypress)) {
            owner->viewOffsets[owner->session()] = 0;
            owner->workspace->setViewOffset(owner->session(), 0);
        }
        owner->workspace->sendInput(owner->session(), { reinterpret_cast<const char*>(bytes), length });
    }
}
void ubq_pixels(UBQWorkspace* owner, uint32_t session, int columns, int rows, int cellWidth, int cellHeight)
{
    owner->cellWidth = cellWidth;
    owner->cellHeight = cellHeight;
    if (owner->workspace)
        owner->workspace->setPixelGeometry(session, { { cellWidth, cellHeight }, { columns * cellWidth, rows * cellHeight }, { columns * cellWidth, rows * cellHeight } });
}
void ubq_mouse(UBQWorkspace* owner, uint32_t session, int kind, int button, int column, int row, int modifiers, int pixelX, int pixelY)
{
    auto* terminal = owner->workspace ? owner->workspace->terminal(session) : nullptr;
    if (!terminal || kind < 0 || kind > 2)
        return;
    std::array<char, 128> bytes {};
    auto count = encodeMouseReport(*terminal, static_cast<Mouse::EventType>(kind), button, row, column, modifiers, bytes, pixelX, pixelY);
    if (count > 0) {
        owner->workspace->sendInput(session, { bytes.data(), static_cast<size_t>(count) });
        return;
    }
    if (kind != 0 || (button != 64 && button != 65))
        return;
    const auto route = Modes::wheelRoute(Mouse::tracksPointer(terminal), Modes::alternateScrollActive(terminal));
    if (route == Modes::WheelRoute::AlternateScrollKeys) {
        const auto size = Modes::alternateScrollKeys(terminal->modes.appCursorKeys ? Modes::CursorKeyMode::Application : Modes::CursorKeyMode::Normal,
            button == 64 ? Modes::ScrollDirection::Up : Modes::ScrollDirection::Down, bytes);
        owner->workspace->sendInput(session, { bytes.data(), static_cast<size_t>(size) });
    } else if (route == Modes::WheelRoute::ScrollView) {
        auto& offset = owner->viewOffsets[session];
        offset = Screen::resolvedViewOffset(terminal, std::max(0, offset + (button == 64 ? 3 : -3)));
        owner->workspace->setViewOffset(session, offset);
        owner->dirty = true;
    }
}
UBQImage ubq_image(UBQWorkspace* owner, uint32_t session, uint32_t imageID)
{
    auto* terminal = owner->workspace ? owner->workspace->terminal(session) : nullptr;
    if (terminal) {
        for (const auto& image : terminal->images.stored) {
            if (image.used && image.imageID == imageID)
                return { image.pixels, image.pixelLength, Image::imageContentStamp(image), image.width, image.height, image.format };
        }
    }
    return {};
}
void ubq_watch_directory(UBQWorkspace* owner)
{
    if (owner->workspace && !owner->workspace->directoryIsLive())
        owner->workspace->watchDirectory();
}
void ubq_set_view(UBQWorkspace* owner, uint32_t session, int columns, int rows, int focused)
{
    if (owner->workspace)
        owner->workspace->setView(session, columns, rows, focused ? Modes::FocusState::Focused : Modes::FocusState::Unfocused);
}
int ubq_intent(UBQWorkspace* owner, const UBQIntent* value)
{
    using Kind = Session::WorkspaceIntentKind;
    static constexpr std::pair<std::string_view, Kind> kinds[] = {
        { "add_tab", Kind::AddTab }, { "set_active_tab", Kind::SetActiveTab },
        { "add_window", Kind::AddSurface }, { "set_active_window", Kind::SetActiveSurface },
        { "focus_pane", Kind::FocusPane }, { "move_focus", Kind::MoveFocus },
        { "split", Kind::SplitFocusedPane }, { "zoom", Kind::ToggleZoom },
        { "balance", Kind::BalanceActiveTab }, { "adjust_ratio", Kind::AdjustFocusedPaneRatio },
        { "extract", Kind::ExtractPaneToTab }, { "swap", Kind::SwapPanes }, { "move", Kind::MovePaneToTab },
        { "rename_tab", Kind::RenameTab }, { "rename_workspace", Kind::RenameWorkspace },
        { "close_pane", Kind::ClosePane }, { "close_tab", Kind::CloseTab }, { "close_window", Kind::CloseSurface }
    };
    const auto found = std::ranges::find(kinds, std::string_view(value->verb), &std::pair<std::string_view, Kind>::first);
    if (found == std::end(kinds))
        return false;
    Session::WorkspaceIntent request;
    request.verb = found->second;
    request.surfaceID = value->surface_id;
    request.tabID = value->tab_id;
    request.paneID = value->pane_id;
    request.targetTabID = value->target_tab_id;
    request.targetPaneID = value->target_pane_id;
    request.direction = value->direction;
    request.orientation = value->orientation;
    request.side = value->side;
    request.ratio = value->ratio;
    request.title.assign(reinterpret_cast<const char*>(value->title), value->title_length);
    owner->send(std::move(request));
    return owner->notice.empty();
}
void ubq_reveal(UBQWorkspace* owner, uint32_t session)
{
    // Focuses a pane of the attached workspace, across windows and tabs,
    // without reconnecting.
    owner->reveal = session;
    owner->revealRevision = 0;
    owner->dirty = true;
}
void ubq_attach(UBQWorkspace* owner, uint32_t session, uint32_t workspace)
{
    // The picker lists the connected daemon, so a workspace it names is pinned there.
    const auto instance = owner->workspace && workspace && !session ? owner->workspace->daemonInstanceID() : 0;
    owner->attach(session, workspace, {}, instance);
}
UBQFrame ubq_frame(UBQWorkspace* owner, uint32_t session)
{
    auto* terminal = owner->workspace ? owner->workspace->terminal(session) : nullptr;
    if (!terminal)
        return {};
    owner->text.clear();
    owner->text.reserve(static_cast<size_t>(terminal->columns) * terminal->rows);
    owner->cells.clear();
    const auto palette = Rendering::narrowPalette(terminal->palette);
    const auto viewOffset = Screen::resolvedViewOffset(terminal, owner->viewOffsets[session]);
    std::vector<Cell> row(static_cast<size_t>(terminal->columns));
    for (int y = 0; y < terminal->rows; ++y) {
        Screen::getDisplayRow(*terminal, viewOffset, y, row.data(), terminal->columns);
        for (const auto& cell : row) {
            const auto& pen = cell.pen;
            auto colors = Rendering::resolveCell(pen, palette, isReverseVideo(terminal), true);
            auto packed = [](Rendering::RGB8 color) -> uint32_t { return (uint32_t(color.red) << 16) | (uint32_t(color.green) << 8) | color.blue; };
            std::string symbol;
            std::array<uint32_t, clusterPoolMaximumCodePoints> points {};
            int count = cellClusterCodePoints(terminal, &cell, points.data(), static_cast<int>(points.size()));
            for (int index = 0; index < count; ++index)
                symbol += encodeUTF8(points[index]);
            if (symbol.empty())
                symbol = encodeUTF8(cell.codePoint ? cell.codePoint : U' ');
            bool continuation = pen.attributes.contains(VT::Attribute::WideContinuation);
            if (continuation || pen.attributes.contains(VT::Attribute::Hidden) || cell.codePoint == 0x10EEEE)
                symbol = " ";
            owner->text.push_back(std::move(symbol));
            uint8_t attributes = 0;
            if (pen.attributes.contains(VT::Attribute::Bold))
                attributes |= 1;
            if (pen.attributes.contains(VT::Attribute::Underline))
                attributes |= 2;
            if (pen.attributes.contains(VT::Attribute::Blink))
                attributes |= 4;
            if (pen.attributes.contains(VT::Attribute::Italic))
                attributes |= 16;
            if (pen.attributes.contains(VT::Attribute::Strike))
                attributes |= 32;
            const auto& text = owner->text.back();
            const auto* hyperlink = Attributes::Hyperlinks::uri(terminal, cell.linkID());
            owner->cells.push_back({ text.data(), text.size(), packed(colors.foreground), packed(colors.background), packed(pen.attributes.contains(VT::Attribute::Underline) ? Rendering::resolveUnderlineColor(pen, terminal->palette, colors.foreground) : colors.foreground), attributes,
                static_cast<uint8_t>(continuation ? 0 : pen.attributes.contains(VT::Attribute::Wide) ? 2
                                                                                                     : 1),
                penUnderlineShape(&pen), hyperlink, hyperlink ? std::strlen(hyperlink) : 0 });
        }
    }
    owner->imageQuads.resize(std::ranges::any_of(terminal->images.stored, [](const auto& image) { return image.used; }) ? static_cast<size_t>(terminal->columns) * terminal->rows + terminal->images.placements.size() : 0);
    const auto count = Rendering::collectImageQuadsOrdered(terminal, { static_cast<float>(owner->cellWidth), static_cast<float>(owner->cellHeight) }, {}, 1, viewOffset, owner->imageQuads,
        { static_cast<float>(terminal->columns * owner->cellWidth), static_cast<float>(terminal->rows * owner->cellHeight) });
    owner->quads.clear();
    for (const auto& quad : std::span(owner->imageQuads).first(static_cast<size_t>(count)))
        owner->quads.push_back({ quad.imageID, quad.minXInPixels, quad.minYInPixels, quad.maxXInPixels - quad.minXInPixels, quad.maxYInPixels - quad.minYInPixels,
            quad.u0, quad.v0, quad.u1, quad.v1, quad.zIndex, static_cast<uint8_t>(quad.filter) });
    const auto* screen = terminal->screens.active;
    return { owner->cells.data(), owner->cells.size(), terminal->columns, terminal->rows, screen ? screen->cursorX : 0, screen ? screen->cursorY : 0,
        static_cast<uint8_t>(terminal->modes.isCursorVisible && !viewOffset), terminal->modes.appCursorKeys, terminal->modes.appKeypad,
        terminal->modes.bracketedPaste, static_cast<uint8_t>(terminal->cursorStyle), terminal->kittyKeyboardFlags[terminal->screens.isUsingAlternateScreen ? 1 : 0], owner->quads.data(), owner->quads.size() };
}
char* ubq_dump(const uint8_t* bytes, size_t length, int columns, int rows, int metadata)
{
    if (columns < 20 || columns > 512 || rows < 4 || rows > 256 || length > 4 * 1024 * 1024)
        return nullptr;
    installPAL();
    auto terminal = std::make_unique<Terminal>();
    resize(*terminal, rows, columns);
    processBytes(terminal.get(), { reinterpret_cast<const char*>(bytes), length });
    std::string output;
    if (metadata) {
        const auto* primary = terminal->programStatus.primary();
        output = "state=" + std::to_string(primary ? static_cast<int>(primary->state) : 0) + "\nkind=" + std::to_string(primary ? static_cast<int>(primary->kind) : 0) + "\nprogress=" + std::to_string(terminal->progressState) + "\n" + std::string(terminal->titleUTF8.view()) + "\n";
    } else {
        std::vector<Cell> row(static_cast<size_t>(columns));
        for (int y = 0; y < rows; ++y) {
            Screen::getDisplayRow(*terminal, 0, y, row.data(), columns);
            for (const auto& cell : row)
                output += encodeUTF8(cell.codePoint ? cell.codePoint : U' ');
            output += '\n';
        }
    }
    return copyText(output);
}
char* ubq_directory(const char* host, const char* ssh, const char* socket, int timeoutMilliseconds)
{
    installPAL();
    auto factory = UbiquittyKit::makeHostChannelFactory({ host, ssh, socket });
    if (!factory)
        return copyText("{\"error\":" + quoted(factory.error()) + "}");
    auto dispatcher = std::make_unique<Dispatcher>();
    if (!dispatcher->isValid())
        return copyText("{\"error\":\"Could not create Core event wakeup pipe\"}");
    DirectoryAnswer answer;
    std::string daemonSocket;
    std::uint64_t daemonInstance = 0;
    {
        // Destroyed before the dispatcher and factory it borrows.
        Session::SessionDirectoryCore directory(dispatcher.get(), factory->get(), &answer);
        if (!directory.refresh()) {
            answer.error = "The workspace listing could not start";
            answer.done = true;
        }
        const auto deadline = Clock::now() + std::chrono::milliseconds(std::max(0, timeoutMilliseconds));
        while (!answer.done) {
            const auto remaining = std::chrono::ceil<std::chrono::milliseconds>(deadline - Clock::now()).count();
            if (remaining <= 0) {
                answer.error = "The host did not answer the workspace listing";
                break;
            }
            const int pending = dispatcher->timeoutMilliseconds();
            pollfd wakeup { dispatcher->descriptor(), POLLIN, 0 };
            ::poll(&wakeup, 1, static_cast<int>(pending < 0 ? remaining : std::min<long long>(pending, remaining)));
            dispatcher->drain();
        }
        daemonSocket = directory.daemonSocketName();
        daemonInstance = directory.daemonInstanceID();
    }
    if (!answer.error.empty() || !answer.tree)
        return copyText("{\"error\":" + quoted(answer.error.empty() ? "The host does not list its workspaces" : answer.error) + "}");
    return copyText(directoryJSON(*answer.tree, daemonSocket, daemonInstance));
}
int ubq_retire_idle_daemon(const char* host, const char* socket)
{
    // Mirrors Core's retirement of an idle bundled host: the process serving
    // the socket is identified by the connection's peer credentials, must run
    // as this user from the host file itself, and is asked with SIGUSR2,
    // which a daemon holding any session or workspace declines.
    if (!host || !socket || !Session::isDaemonSocketName(socket))
        return 0;
    std::array<char, 1024> path {};
    sockaddr_un address {};
    const auto length = Session::controlFilePath(socket, path);
    if (!length || length >= sizeof(address.sun_path))
        return 0;
    address.sun_family = AF_UNIX;
    std::memcpy(address.sun_path, path.data(), length + 1);
    pid_t process = 0;
    const int channel = ::socket(AF_UNIX, SOCK_STREAM, 0);
    if (channel < 0)
        return 0;
    bool isOwn = ::fcntl(channel, F_SETFD, FD_CLOEXEC) == 0
        && ::connect(channel, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) == 0;
#if defined(__APPLE__)
    uid_t user = 0;
    gid_t group = 0;
    socklen_t size = sizeof(process);
    isOwn = isOwn && ::getpeereid(channel, &user, &group) == 0 && user == ::geteuid()
        && ::getsockopt(channel, SOL_LOCAL, LOCAL_PEERPID, &process, &size) == 0;
#elif defined(__linux__)
    ucred credentials {};
    socklen_t size = sizeof(credentials);
    isOwn = isOwn && ::getsockopt(channel, SOL_SOCKET, SO_PEERCRED, &credentials, &size) == 0 && credentials.uid == ::geteuid();
    process = credentials.pid;
#else
    isOwn = false;
#endif
    // Closed before the signal: the daemon defers an idle stop while any
    // connection has yet to greet.
    ::close(channel);
    if (!isOwn || process <= 0)
        return 0;
    struct stat expected { }, running { };
    if (::stat(host, &expected) != 0)
        return 0;
#if defined(__APPLE__)
    std::array<char, PROC_PIDPATHINFO_MAXSIZE> image {};
    if (::proc_pidpath(process, image.data(), image.size()) <= 0 || ::stat(image.data(), &running) != 0)
        return 0;
#else
    if (::stat(("/proc/" + std::to_string(process) + "/exe").c_str(), &running) != 0)
        return 0;
#endif
    if (running.st_dev != expected.st_dev || running.st_ino != expected.st_ino)
        return 0;
    return ::kill(process, SIGUSR2) == 0;
}
void ubq_text_free(char* text) { delete[] text; }
}

namespace Ubiquitty::PAL {
Ticks monotonicTicks() { return Ticks::fromMilliseconds(static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now().time_since_epoch()).count())); }
uint32_t monotonicMicros() { return static_cast<uint32_t>(std::chrono::duration_cast<std::chrono::microseconds>(Clock::now().time_since_epoch()).count()); }
void diagWrite(std::string_view) { }
void bellRing(BellMode) { }
void clipboardSetText(std::string_view) { }
int clipboardGetText(char*, int) { return -1; }
int preferencesLoad(PreferencesBlobKey, void*, int) { return -1; }
int preferencesSaveAtomic(PreferencesBlobKey, std::span<const std::byte>) { return 0; }
}
