export class Network {
  constructor() {
    this.ws = null;
    this.listeners = {};
    this.connected = false;
  }

  connect() {
    return new Promise((resolve, reject) => {
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      this.ws = new WebSocket(`${proto}//${location.host}`);

      this.ws.onopen = () => {
        this.connected = true;
        resolve();
      };

      this.ws.onerror = () => reject(new Error('WebSocket connection failed'));

      this.ws.onclose = () => {
        this.connected = false;
        this.emit('disconnected');
      };

      this.ws.onmessage = (event) => {
        let msg;
        try { msg = JSON.parse(event.data); } catch { return; }
        this.emit(msg.t, msg);
      };
    });
  }

  on(event, fn) {
    if (!this.listeners[event]) this.listeners[event] = [];
    this.listeners[event].push(fn);
  }

  off(event, fn) {
    if (!this.listeners[event]) return;
    if (fn === null) {
      delete this.listeners[event];
    } else {
      this.listeners[event] = this.listeners[event].filter(f => f !== fn);
    }
  }

  emit(event, data) {
    if (!this.listeners[event]) return;
    for (const fn of this.listeners[event]) fn(data);
  }

  send(obj) {
    if (this.ws && this.ws.readyState === 1) {
      this.ws.send(JSON.stringify(obj));
    }
  }

  createRoom() {
    this.send({ t: 'create_room' });
  }

  joinRoom(code) {
    this.send({ t: 'join_room', code });
  }

  sendInput(heldControls) {
    this.send({ t: 'input', h: heldControls });
  }

  requestRematch() {
    this.send({ t: 'rematch_request' });
  }

  disconnect() {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.connected = false;
    this.listeners = {};
  }
}
