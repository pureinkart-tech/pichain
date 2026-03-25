const express = require('express')
const socketio = require('socket.io')
const http = require('http')
const cors = require('cors')
const { addUser, removeUser, getUser, getUsersInRoom } = require('./users')
const path = require('path')

const PORT = process.env.PORT || 5000

const app = express()
const server = http.createServer(app)
const io = socketio(server, {
    cors: {
        origin: '*',
        methods: ['GET', 'POST']
    }
})

app.use(cors())
app.use(express.json())

// REST: POST /add-bot — spawn AI bots using the external uno-bot.js
app.post('/add-bot', (req, res) => {
    try {
        const { roomId, count } = req.body || {};
        if (!roomId) return res.status(400).json({ error: 'roomId required' });
        const numBots = Math.min(parseInt(count) || 1, 10);
        const { spawn } = require('child_process');
        const botPath = require('path');
        const botScript = botPath.resolve(__dirname, '../../../test-bots/uno-bot.js');
        const bot = spawn('node', [botScript, roomId, String(numBots)], {
            stdio: ['ignore', 'pipe', 'pipe'],
        });
        bot.stdout.on('data', d => console.log('[BOT] ' + d.toString().trim()));
        bot.stderr.on('data', d => console.error('[BOT-ERR] ' + d.toString().trim()));
        bot.on('close', code => console.log(`[BOT] UNO bot process exited with code ${code}`));
        console.log(`[UNO] Spawned ${numBots} AI bot(s) for room ${roomId} (pid: ${bot.pid})`);
        res.json({ ok: true, roomId, bots: numBots, pid: bot.pid });
    } catch (err) {
        console.error('add-bot error:', err);
        res.status(500).json({ error: err.message });
    }
});

io.on('connection', socket => {
    console.log('[UNO] Client connected:', socket.id);
    socket.on('join', (payload, callback) => {
        console.log('[UNO] Join request:', payload);
        let usersInRoom = getUsersInRoom(payload.room)
        let playerNumber = usersInRoom.length + 1

        const { error, newUser} = addUser({
            id: socket.id,
            name: 'Player ' + playerNumber,
            room: payload.room
        })

        if(error)
            return callback(error)

        socket.join(newUser.room)

        io.to(newUser.room).emit('roomData', {room: newUser.room, users: getUsersInRoom(newUser.room)})
        socket.emit('currentUserData', {name: newUser.name})
        callback()
    })

    socket.on('initGameState', gameState => {
        const user = getUser(socket.id)
        if(user)
            io.to(user.room).emit('initGameState', gameState)
    })

    socket.on('updateGameState', gameState => {
        const user = getUser(socket.id)
        if(user)
            io.to(user.room).emit('updateGameState', gameState)
    })

    socket.on('sendMessage', (payload, callback) => {
        const user = getUser(socket.id)
        io.to(user.room).emit('message', {user: user.name, text: payload.message})
        callback()
    })

    socket.on('disconnect', (reason) => {
        console.log('[UNO] Client disconnected:', socket.id, reason);
        const user = removeUser(socket.id)
        if(user)
            io.to(user.room).emit('roomData', {room: user.room, users: getUsersInRoom(user.room)})
    })
})

//serve static assets in production
if(process.env.NODE_ENV === 'production') {
	//set static folder
	app.use(express.static('client/build'))
	app.get('*', (req, res) => {
		res.sendFile(path.resolve(__dirname, 'client', 'build', 'index.html'))
	})
}

server.listen(PORT, () => {
    console.log(`Server running on port ${PORT}`)
})
