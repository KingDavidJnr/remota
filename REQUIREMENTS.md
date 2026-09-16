# Remota

## Product Requirements Document

**Project:** Remota
**Version:** 1.0
**Status:** Initial Requirements
**Primary Target:** Web + Desktop
**Future Targets:** Android + iOS

---

## 1. Overview

Remota is a lightweight remote collaboration application that allows one person to create a temporary connection link and share it with another person.

The recipient opens the link, explicitly grants the required permissions, and establishes a real-time connection with the initiator. Once connected, the initiator can view and interact with the recipient's computer remotely, including controlling the mouse and keyboard and operating applications on the remote device.

Remota is designed around a simple interaction model:

> **Create link → Share link → Accept → Connect → Control → End**

The initial release will focus on computer-to-computer remote control through a web-based controller and a native desktop endpoint. Mobile support will be developed as a subsequent roadmap item.

---

# 2. Goals

The primary goals of Remota are:

1. Allow a user to create a remote connection without creating an account.
2. Generate a unique shareable link for each connection.
3. Allow another person to join through the link.
4. Require explicit consent from the remote participant before access is granted.
5. Establish a low-latency real-time connection between both parties.
6. Allow the initiator to view the remote computer's screen.
7. Allow the initiator to control the remote computer's mouse and keyboard.
8. Allow the initiator to interact with applications running on the remote computer.
9. Allow either participant to terminate the connection immediately.
10. Persist active room information in a database.
11. Keep the architecture extensible for future Android and iOS clients.

---

# 3. Non-Goals for Initial Release

The initial release will not include:

* User accounts
* Authentication
* Single Sign-On
* Organization management
* Employee management
* Administrative dashboards
* Complex role-based permissions
* Permanent device registration
* Session recording
* File transfer
* Voice/video calling
* In-app chat
* Remote printing
* Mobile applications
* Linux support
* Automated unattended remote access

These may be considered in future versions where required.

---

# 4. Core User Roles

Remota has two participants in a connection.

## 4.1 Controller

The Controller is the person who creates the connection.

The Controller can:

* Create a connection.
* Obtain a shareable link.
* Share the link with another person.
* Wait for the participant to join.
* View the participant's screen.
* Control the participant's mouse.
* Control the participant's keyboard.
* Interact with applications on the remote computer.
* End the connection.

## 4.2 Participant

The Participant is the person receiving the connection request.

The Participant can:

* Open the shared connection link.
* View information about the connection.
* Explicitly approve or reject the connection.
* Grant required operating-system permissions.
* Share their screen.
* Allow remote mouse and keyboard control.
* See that a remote connection is active.
* Terminate the connection at any time.

---

# 5. Primary User Flow

## 5.1 Create Connection

1. Controller opens Remota.
2. Controller selects **Create Connection**.
3. Remota requests a new room from the backend.
4. Backend generates a cryptographically secure room identifier/token.
5. Room is persisted in the database.
6. Remota displays a shareable link.
7. Controller sends the link to the Participant.

Example:

```text
https://remota.example.com/join/7f4c9a...
```

---

## 5.2 Join Connection

1. Participant opens the link.
2. Remota validates the room.
3. If the room is valid, the Participant sees a connection consent screen.
4. The Participant is informed that their computer will be remotely accessed.
5. Participant explicitly accepts.
6. Participant opens or launches the required Remota desktop endpoint.
7. The endpoint requests the necessary operating-system permissions.
8. Participant grants the required permissions.
9. WebRTC negotiation begins.
10. The connection becomes active.

---

## 5.3 Active Connection

Once connected:

```text
Participant Computer
        │
        ├── Screen
        ├── Mouse
        └── Keyboard
              │
            WebRTC
              │
              ▼
       Controller Browser
```

The Controller should be able to:

* See the remote screen.
* Move the remote mouse.
* Click.
* Double-click.
* Right-click.
* Scroll.
* Drag.
* Type.
* Use keyboard shortcuts.
* Open applications.
* Interact with applications.
* Resize or adapt the remote viewport.

---

## 5.4 End Connection

Either participant can terminate the connection.

When terminated:

1. WebRTC connection is closed.
2. WebSocket connection is closed.
3. Remote-control capability is revoked.
4. Desktop endpoint returns to an inactive state.
5. Room status is updated to `ENDED`.
6. Active connection resources are released.

---

# 6. Room Requirements

A room represents a temporary remote connection.

A room should contain at minimum:

```text
id
token
status
createdAt
expiresAt
```

Possible statuses:

```text
WAITING
CONNECTING
ACTIVE
ENDED
EXPIRED
```

## 6.1 Room Token

Room tokens must:

* Be cryptographically generated.
* Be sufficiently long and unpredictable.
* Not contain sequential identifiers.
* Be safe to include in a URL.
* Be associated with exactly one room.
* Become invalid when the room expires or is terminated.

## 6.2 Room Expiration

Rooms must have an expiration mechanism.

A room that remains unused beyond its expiration period should become:

```text
EXPIRED
```

Expired rooms must not allow new participants to connect.

---

# 7. No-Account Model

Remota does not require user accounts for the initial release.

The Controller creates a temporary room and receives a unique link.

The Participant does not need to register or log in.

Possession of the invitation link allows a person to request access to the room, but **does not automatically grant remote access**.

---

# 8. Web Application Requirements

The web application will be built using:

* React
* TypeScript
* Vite
* Tailwind CSS
* Native WebRTC browser APIs
* WebSocket client

## 8.1 Home Screen

The home screen must provide:

```text
Create Connection
```

After creation, the Controller should immediately receive the connection link.

## 8.2 Waiting Screen

While waiting for the Participant:

```text
Waiting for participant...

Share this link:
https://remota.example.com/join/...

[Copy Link]
[End Connection]
```

## 8.3 Remote Desktop Screen

Once connected:

```text
┌──────────────────────────────────────┐
│ Connected                            │
├──────────────────────────────────────┤
│                                      │
│          Remote Computer             │
│                                      │
│          Screen Stream               │
│                                      │
├──────────────────────────────────────┤
│ Connection status       ● Connected  │
│                                      │
│ [End Connection]                     │
└──────────────────────────────────────┘
```

The remote screen should receive pointer and keyboard interaction.

---

# 9. Participant Web Experience

When the Participant opens a valid link, they should see a clear consent screen.

Example:

```text
Someone wants to connect to your computer.

By accepting, you will allow the other person
to view and interact with your computer.

[ Accept Connection ]

[ Decline ]
```

The interface must clearly communicate that remote control is being granted.

The Participant must never be connected automatically.

---

# 10. Desktop Endpoint

The desktop endpoint is responsible for capabilities that cannot safely or reliably be performed by a normal browser.

The initial desktop target is:

**Windows**

The desktop endpoint should be implemented as a native application.

Preferred implementation:

**Rust**

## 10.1 Endpoint Responsibilities

The endpoint must provide:

* Session connection
* WebRTC communication
* Screen capture
* Mouse input
* Keyboard input
* Permission handling
* Connection lifecycle
* Session termination

Future capabilities may include:

* Clipboard
* File transfer
* Audio
* Multi-monitor support
* System tray integration
* macOS support

---

# 11. Screen Sharing

The remote endpoint must capture the participant's computer screen and transmit it to the Controller.

Requirements:

* Low latency.
* Adaptive resolution.
* Adaptive bitrate.
* Efficient video encoding.
* Ability to handle changing network conditions.
* Screen capture must stop immediately when the connection ends.

The preferred transport is WebRTC.

---

# 12. Remote Mouse Control

The Controller must be able to interact with the remote mouse.

Supported operations should include:

* Mouse movement
* Left click
* Right click
* Double click
* Mouse button down
* Mouse button up
* Drag
* Scroll

Pointer coordinates should preferably be transmitted using normalized coordinates:

```text
x: 0.0 - 1.0
y: 0.0 - 1.0
```

This allows the remote display to be rendered at different sizes without changing the coordinate protocol.

---

# 13. Remote Keyboard Control

The Controller must be able to send keyboard events to the remote endpoint.

The implementation must support:

* Standard alphanumeric keys
* Modifier keys
* Function keys
* Arrow keys
* Navigation keys
* Special keys
* Keyboard shortcuts
* Key-down events
* Key-up events

The endpoint must correctly maintain keyboard state to prevent stuck keys when connections fail.

---

# 14. Remote Control Protocol

Remote control commands should use a common protocol independent of the endpoint platform.

Example:

```json
{
  "type": "mouse_move",
  "x": 0.42,
  "y": 0.63
}
```

Mouse click:

```json
{
  "type": "mouse_button",
  "action": "down",
  "button": "left"
}
```

Keyboard:

```json
{
  "type": "keyboard",
  "action": "down",
  "key": "A"
}
```

Scroll:

```json
{
  "type": "scroll",
  "deltaX": 0,
  "deltaY": -480
}
```

The protocol should be designed so that future Android and iOS endpoints can implement the same conceptual control messages.

---

# 15. WebRTC

WebRTC will be the primary real-time transport.

WebRTC will provide:

### Media

* Remote screen video
* Future audio

### DataChannel

* Mouse events
* Keyboard events
* Touch events
* Future clipboard
* Future file transfer
* Other control messages

The actual screen and control traffic should not pass through the Express application server when a direct WebRTC connection is possible.

---

# 16. Signaling

The backend will provide WebRTC signaling through WebSockets.

Signaling messages will include:

* SDP offers
* SDP answers
* ICE candidates
* Connection state
* Participant joining
* Participant leaving
* Session termination

The signaling server is responsible for establishing the WebRTC connection but does not act as the primary transport for the remote screen.

---

# 17. STUN/TURN

Remota must support NAT traversal.

The initial infrastructure will use:

**coturn**

The system should attempt a direct peer-to-peer WebRTC connection first.

TURN should be used when a direct connection cannot be established.

---

# 18. Backend Requirements

Backend stack:

* Node.js
* TypeScript
* Express.js
* WebSocket
* Prisma
* PostgreSQL

Backend responsibilities:

```text
Room creation
Room validation
Room expiration
Room status
WebRTC signaling
Connection coordination
Session termination
```

The backend should not implement authentication for the initial release.

---

# 19. Database

PostgreSQL will persist room state.

Initial schema:

```text
Room
────
id
token
status
createdAt
expiresAt
```

A future event/audit table can be introduced if required.

The database should not store:

* Screen video
* Keyboard data
* Mouse data
* Audio streams

Real-time communication belongs to WebRTC.

---

# 20. Security Requirements

Because Remota provides remote computer control, security is a fundamental requirement.

The system must:

1. Require explicit Participant consent.
2. Never automatically grant remote control.
3. Use unpredictable room tokens.
4. Use HTTPS/WSS.
5. Use WebRTC's encrypted transport.
6. Expire inactive rooms.
7. Immediately terminate control when the room ends.
8. Allow the Participant to terminate the connection.
9. Prevent access to expired rooms.
10. Prevent multiple unauthorized controllers from taking control of a room.
11. Clearly indicate when remote control is active.
12. Avoid persistent unattended access in the initial release.

The remote endpoint should not silently start remote-control capabilities without the Participant's knowledge.

---

# 21. Connection States

The UI and backend should recognize the following states:

```text
CREATED
WAITING
PARTICIPANT_JOINED
CONSENT_PENDING
CONNECTING
ACTIVE
DISCONNECTING
ENDED
EXPIRED
```

The UI should provide an appropriate state to the Controller and Participant.

---

# 22. Connection Reliability

The application should handle:

* Temporary network interruption
* WebRTC ICE failures
* Participant disconnects
* Controller disconnects
* Endpoint crashes
* Browser refreshes
* Room expiration
* Duplicate join attempts

When the connection cannot be recovered, the room should be safely terminated.

The endpoint must fail closed, meaning that losing the connection must never leave remote-control input active.

---

# 23. Initial Browser/Desktop Compatibility

Initial development should prioritize:

### Controller

Modern Chromium-based browsers and other modern browsers supporting WebRTC.

### Remote endpoint

Windows desktop.

Support for macOS will follow after the Windows implementation is stable.

---

# 24. Future Mobile Roadmap

Mobile support is part of the product roadmap but is not part of the initial implementation.

## Android

Future Android application:

* Kotlin
* WebRTC
* MediaProjection
* Appropriate Android interaction/accessibility APIs

Capabilities will depend on Android platform permissions and restrictions.

## iOS

Future iOS application:

* Swift
* WebRTC
* ReplayKit
* Apple-approved APIs

iOS capabilities must respect Apple's platform restrictions around remote interaction with other applications and system UI.

The common Remota protocol should therefore be designed to support platform-specific capability sets.

---

# 25. Future Features

Potential future features include:

* Android support
* iOS support
* macOS support
* Linux support
* Clipboard sharing
* File transfer
* Voice communication
* Text chat
* Session recording
* Multi-monitor support
* Better connection diagnostics
* Remote resolution controls
* Device information
* Session history
* Optional authentication
* Organization features
* Administrative controls
* Managed company devices

These features should not complicate the initial MVP.

---

# 26. MVP Acceptance Criteria

The MVP is considered successful when the following complete workflow works reliably:

### Controller

1. Opens Remota.
2. Creates a connection.
3. Receives a unique link.
4. Shares the link.

### Participant

5. Opens the link.
6. Sees the consent screen.
7. Accepts the connection.
8. Opens the desktop endpoint.
9. Grants required operating-system permissions.

### Connection

10. WebRTC establishes successfully.
11. Controller sees the Participant's desktop.
12. Controller can move the remote mouse.
13. Controller can click.
14. Controller can scroll.
15. Controller can type.
16. Controller can open an application.
17. Controller can interact with that application.
18. Participant can see that the connection is active.
19. Participant can terminate the connection.
20. Controller can terminate the connection.
21. Remote control stops immediately after termination.
22. Room status is updated in PostgreSQL.

**The critical MVP demonstration is:**

> A person receives a Remota link, accepts the connection, and the Controller can open and operate applications on their computer remotely.

If that works reliably, the fundamental Remota product has been proven.
