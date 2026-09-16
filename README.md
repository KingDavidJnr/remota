# Remota

**Remota** is a lightweight remote collaboration platform that lets one person connect to and control another person's computer through a simple shared link and explicit consent.

The core experience is:

> **Create link → Share link → Accept → Connect → Control → End**

Remota is being developed as a web-first platform, with desktop support initially and Android/iOS support planned for future releases.

---

## How It Works

### Controller

The person initiating the connection:

1. Opens Remota.
2. Creates a connection.
3. Receives a unique shareable link.
4. Sends the link to another person.
5. Waits for the participant to accept.
6. Views the remote computer.
7. Controls the remote mouse and keyboard.
8. Interacts with applications on the remote computer.
9. Ends the connection when finished.

### Participant

The person receiving the connection:

1. Opens the Remota link.
2. Reviews the connection request.
3. Explicitly accepts the connection.
4. Opens the Remota desktop endpoint.
5. Grants the required operating-system permissions.
6. Allows screen sharing and remote control.
7. Uses the computer normally while the connection is active.
8. Can terminate the connection at any time.

No account or login is required for the initial version.

---

## Core Architecture

```text
                         Remota
                            │
                ┌───────────┴───────────┐
                │                       │
             Web App                 Backend
          React + Vite              Express.js
                │                       │
                │                  WebSocket
                │                  Signaling
                │                       │
                │                   PostgreSQL
                │                       │
                └───────────┬───────────┘
                            │
                         WebRTC
                            │
                 ┌──────────┴──────────┐
                 │                     │
          Controller Browser      Desktop Endpoint
                                      │
                                      │
                              Screen Capture
                              Mouse Control
                              Keyboard Control
```

WebRTC is responsible for the real-time connection.

The backend primarily handles:

* Room creation
* Room validation
* Room lifecycle
* WebRTC signaling

The remote desktop endpoint handles operating-system-level capabilities such as:

* Screen capture
* Mouse input
* Keyboard input

---

## Technology Stack

### Web Application

* React
* TypeScript
* Vite
* Tailwind CSS
* WebRTC
* WebSocket

### Backend

* Node.js
* TypeScript
* Express.js
* WebSocket
* Prisma

### Database

* PostgreSQL

The database stores temporary room information and lifecycle state.

### Real-Time Communication

* WebRTC
* WebRTC DataChannel
* STUN
* TURN
* coturn

### Desktop Endpoint

The initial desktop endpoint targets Windows.

Preferred implementation:

* Rust
* Native Windows APIs
* WebRTC

Future desktop support will include macOS.

---

## Project Structure

```text
remota/
│
├── apps/
│   │
│   ├── web/
│   │   ├── src/
│   │   │   ├── components/
│   │   │   ├── pages/
│   │   │   ├── signaling/
│   │   │   ├── webrtc/
│   │   │   └── remote-control/
│   │   └── package.json
│   │
│   ├── server/
│   │   ├── src/
│   │   │   ├── routes/
│   │   │   ├── rooms/
│   │   │   ├── websocket/
│   │   │   └── index.ts
│   │   └── package.json
│   │
│   └── desktop-agent/
│       ├── src/
│       │   ├── webrtc/
│       │   ├── capture/
│       │   ├── input/
│       │   └── session/
│       └── Cargo.toml
│
├── packages/
│   │
│   └── protocol/
│       └── remote-control-messages
│
├── prisma/
│   └── schema.prisma
│
├── requirements.md
└── README.md
```

The exact structure may evolve during implementation.

---

## Connection Flow

### 1. Create

The Controller requests a new room:

```http
POST /api/rooms
```

The server generates a secure room token and persists the room.

Example:

```text
https://remota.example.com/join/7f4c9a...
```

---

### 2. Join

The Participant opens the link.

The server validates:

* Room existence
* Room status
* Room expiration

If valid, the Participant can proceed to the consent screen.

---

### 3. Consent

The Participant must explicitly approve the connection.

Remote access must never be granted automatically.

---

### 4. Signaling

The Controller and endpoint exchange WebRTC signaling information through the backend.

```text
Controller
    │
    │ WebSocket
    ▼
Express Server
    │
    │ WebSocket
    ▼
Remote Endpoint
```

---

### 5. WebRTC

Once signaling is complete, the peers establish a WebRTC connection.

```text
Controller
     │
     │
     │ WebRTC
     │
     ▼
Remote Endpoint
```

The screen is transmitted as a WebRTC media stream.

Remote-control commands are transmitted through a WebRTC DataChannel.

---

## Remote Control Protocol

Remote control messages use a platform-independent protocol.

Example mouse movement:

```json
{
  "type": "mouse_move",
  "x": 0.42,
  "y": 0.63
}
```

Mouse button:

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

Coordinates should use normalized values so that remote screens can be displayed at different sizes.

The protocol is intentionally platform-independent so that future Windows, macOS, Android and iOS endpoints can implement the same communication model.

---

## Room Model

Remota does not require user accounts for the initial release.

A room represents a temporary connection.

Conceptually:

```text
Room
├── id
├── token
├── status
├── createdAt
└── expiresAt
```

Room states:

```text
WAITING
CONNECTING
ACTIVE
ENDED
EXPIRED
```

When a connection ends, remote-control access must immediately stop.

---

## Security

Remote control requires explicit consent.

Remota must:

* Use unpredictable room tokens.
* Use HTTPS and secure WebSockets.
* Use WebRTC's encrypted transport.
* Require explicit participant approval.
* Allow the participant to terminate the connection.
* Allow the controller to terminate the connection.
* Expire unused rooms.
* Prevent access to expired rooms.
* Prevent unauthorized control of active rooms.
* Stop remote control when the connection terminates.
* Avoid unattended remote access in the initial release.

The Participant should always be able to clearly determine when remote access is active.

---

## MVP

The first milestone is intentionally narrow.

### Controller

* Create connection
* Generate link
* Share link
* Wait for participant
* View remote screen
* Control mouse
* Control keyboard
* End connection

### Participant

* Open link
* Review request
* Accept/decline
* Open desktop endpoint
* Grant permissions
* Share screen
* Allow remote control
* End connection

### Desktop

The initial endpoint will target Windows.

The MVP must demonstrate:

```text
Create link
     ↓
Open link
     ↓
Accept
     ↓
Launch endpoint
     ↓
Grant permissions
     ↓
Connect
     ↓
View desktop
     ↓
Move mouse
     ↓
Click
     ↓
Type
     ↓
Open application
     ↓
Interact with application
     ↓
End connection
```

---

## Development Roadmap

### Phase 1 — WebRTC Proof of Concept

Establish a basic browser-to-browser WebRTC connection and verify:

* Signaling
* Screen sharing
* DataChannel communication

### Phase 2 — Windows Remote Control

Introduce the native Windows endpoint.

Implement:

* Screen capture
* Mouse movement
* Mouse buttons
* Keyboard
* Scroll
* Connection lifecycle

### Phase 3 — Link-Based Product Flow

Implement the complete Remota experience:

```text
Create
  ↓
Share
  ↓
Join
  ↓
Consent
  ↓
Connect
  ↓
Control
  ↓
End
```

### Phase 4 — Reliability

Implement:

* TURN
* Connection recovery
* ICE handling
* Network adaptation
* Resolution adaptation
* Connection status
* Room expiration
* Cleanup

### Phase 5 — macOS

Add a native macOS endpoint.

### Phase 6 — Android

Add a native Android application.

### Phase 7 — iOS

Add a native iOS application while respecting Apple's platform capabilities and restrictions.

---

## Future Features

Potential future capabilities include:

* Clipboard sharing
* File transfer
* Voice communication
* Text chat
* Session recording
* Multi-monitor support
* Better connection diagnostics
* Remote resolution controls
* Device information
* Optional authentication
* Managed devices
* Organization features

These are outside the initial MVP.

---

## Design Principles

### Simple

A user should not need an account to initiate a temporary connection.

### Consent-driven

The person whose device is being accessed must explicitly approve the connection.

### Temporary

Connections are temporary and should expire or terminate cleanly.

### Low latency

Remote interaction should feel as close to local interaction as network conditions allow.

### Platform-aware

The same Remota experience should span platforms while respecting the capabilities and restrictions of each operating system.

### Extensible

The communication protocol and backend should allow future endpoint implementations without redesigning the core product.

---

## Requirements

See [`requirements.md`](./requirements.md) for the complete product requirements and acceptance criteria.

---

## Project Status

**Status:** Initial development

**Current focus:**

> Web controller + Express signaling server + PostgreSQL room management + WebRTC + Windows desktop endpoint.

Android and iOS are planned for future releases.
