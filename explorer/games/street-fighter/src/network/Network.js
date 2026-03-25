export class Network {
  constructor() {
    this.ws = null;
    this.listeners = {};
    this.connected = false;
  }

  connect() {
    return new Promise((resolve, reject) => {
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      const relayUrl = location.hostname.includes('pichain.net')
        ? `${proto}//${location.hostname}/ws/relay`
        : `${proto}//${location.hostname}:8403`;
      this.ws = new WebSocket(relayUrl);

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

  createRoom(code = null) {
    const payload = { t: 'create_room' };
    if (code) payload.code = code;
    this.send(payload);
  }

  joinRoom(code) {
    this.send({ t: 'join_room', code });
  }

  joinMatch(matchId) {
    this.send({ t: 'sf_join_match', match_id: String(matchId || '') });
  }

  sendInput(heldControls) {
    this.send({ t: 'input', h: heldControls });
  }

  sendHealthSync(hp0, hp1) {
    this.send({ t: 'hs', hp: [hp0, hp1] });
  }

  sendGameResult(winnerId) {
    this.send({ t: 'gr', winner: winnerId });
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
