import asyncio
import json
import websockets
import socket
import gi
gi.require_version('Gst', '1.0')
gi.require_version('GstWebRTC', '1.0')
gi.require_version('GstSdp', '1.0')
from gi.repository import Gst, GObject, GstWebRTC, GstSdp

class SignalingServer:
    def __init__(self, webrtcbin, host='0.0.0.0', port=8081):
        self.webrtcbin = webrtcbin
        self.host = host
        self.port = port
        self.server = None
        self.connections = set()
        self.running = False
        self.offer_in_progress = False

        # Connect to webrtcbin signals
        self.webrtcbin.connect('on-negotiation-needed', self.on_negotiation_needed)
        self.webrtcbin.connect('on-ice-candidate', self.on_ice_candidate)

    async def run(self):
        """Start the signaling server."""
        print(f"[SignalingServer] Starting server on {self.host}:{self.port}")
        if self.running:
            print("[SignalingServer] Server is already running!")
            return

        self.running = True
        try:
            # Create server
            print("[SignalingServer] Starting WebSocket server...")
            server = await websockets.serve(
                self.handle_connection,
                self.host,
                self.port,
                reuse_address=True
            )
            print("[SignalingServer] WebSocket server started successfully")

            # Keep the server running
            self.exit_future = asyncio.Future()
            await self.exit_future
            print("[SignalingServer] Server stopped")
        except Exception as e:
            print(f"[SignalingServer] Failed to start server: {e}")
        finally:
            self.running = False
            if hasattr(self, 'exit_future'):
                self.exit_future = None

    def start(self):
        """Start the server in a new event loop."""
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        try:
            loop.run_until_complete(self.run())
        finally:
            loop.close()

    def stop(self):
        """Stop the signaling server."""
        print("[SignalingServer] Stopping server...")
        self.running = False

        if hasattr(self, 'exit_future') and not self.exit_future.done():
            print("[SignalingServer] Triggering server shutdown...")
            asyncio.run_coroutine_threadsafe(
                self.close_all_connections(),
                self.exit_future.get_loop()
            )
            self.exit_future.set_result(None)

    async def close_all_connections(self):
        """Close all active WebSocket connections."""
        if self.connections:
            print(f"[SignalingServer] Closing {len(self.connections)} active connections...")
            close_tasks = [ws.close() for ws in self.connections]
            await asyncio.gather(*close_tasks, return_exceptions=True)
            self.connections.clear()
            print("[SignalingServer] All connections closed")

    async def hello_peer(self, websocket):
        """Exchange hello messages with peer."""
        try:
            # Wait for HELLO from peer
            hello = await websocket.recv()
            print(f"[SignalingServer] Received initial message: {hello}")

            if hello != 'HELLO':
                print("[SignalingServer] Invalid hello message")
                await websocket.close(code=1002, reason='invalid protocol')
                return None

            # Send back HELLO
            await websocket.send('HELLO')
            print("[SignalingServer] Sent HELLO response")

            # After successful handshake, send the current offer if we have one
            if self.offer_in_progress:
                print("[SignalingServer] Waiting for offer to complete before sending...")
                await asyncio.sleep(1)  # Give time for offer to complete

            if hasattr(self, 'current_offer'):
                print("[SignalingServer] Sending current offer to new peer")
                await websocket.send(json.dumps({
                    'type': 'offer',
                    'sdp': self.current_offer
                }))

            return True
        except Exception as e:
            print(f"[SignalingServer] Error in hello exchange: {e}")
            return None

    async def handle_connection(self, websocket, path):
        """Handle a new WebSocket connection."""
        client_id = id(websocket)
        print(f"[SignalingServer] New client connecting... (ID: {client_id})")
        try:
            # Wait for HELLO
            hello = await websocket.recv()
            print(f"[SignalingServer] Received initial message: {hello}")
            if not hello.startswith('HELLO'):
                print("[SignalingServer] Invalid hello message")
                await websocket.close(code=1002, reason='invalid protocol')
                return

            # Send back HELLO
            await websocket.send('HELLO')
            print("[SignalingServer] Sent HELLO response")

            # Wait for ROOM command
            room_cmd = await websocket.recv()
            print(f"[SignalingServer] Received room command: {room_cmd}")
            if not room_cmd.startswith('ROOM'):
                print("[SignalingServer] Invalid room command")
                await websocket.close(code=1002, reason='invalid protocol')
                return

            # Send ROOM_OK
            await websocket.send('ROOM_OK')
            print("[SignalingServer] Sent ROOM_OK")

            # Add to connections and send current offer if we have one
            self.connections.add(websocket)
            print(f"[SignalingServer] Client {client_id} connected successfully")
            print(f"[SignalingServer] Total active connections: {len(self.connections)}")

            if hasattr(self, 'current_offer'):
                print("[SignalingServer] Sending current offer to new peer")
                message = json.dumps({
                    'offer': {
                        'type': 'offer',
                        'sdp': self.current_offer
                    }
                })
                await websocket.send(f"ROOM_PEER_MSG server {message}")

            async for message in websocket:
                if message.startswith('ROOM_PEER_MSG'):
                    jsonStart = message.find('{')
                    if jsonStart == -1:
                        continue
                    try:
                        data = json.loads(message[jsonStart:])
                        print(f"[SignalingServer] Received peer message from client {client_id}:", data)

                        if 'answer' in data:
                            print(f"[SignalingServer] Processing answer SDP from client {client_id}")
                            try:
                                _, sdpmsg = GstSdp.SDPMessage.new()
                                GstSdp.sdp_message_parse_buffer(bytes(data['answer']['sdp'].encode()), sdpmsg)
                                answer = GstWebRTC.WebRTCSessionDescription.new(GstWebRTC.WebRTCSDPType.ANSWER, sdpmsg)
                                promise = Gst.Promise.new()
                                self.webrtcbin.emit('set-remote-description', answer, promise)
                                promise.interrupt()
                                print(f"[SignalingServer] Successfully set remote description for client {client_id}")
                            except Exception as e:
                                print(f"[SignalingServer] Error setting remote description: {e}")

                        elif 'iceCandidate' in data:
                            print(f"[SignalingServer] Processing ICE candidate from client {client_id}")
                            try:
                                self.webrtcbin.emit('add-ice-candidate',
                                                data['iceCandidate']['sdpMLineIndex'],
                                                data['iceCandidate']['candidate'])
                                print(f"[SignalingServer] Successfully added ICE candidate from client {client_id}")
                            except Exception as e:
                                print(f"[SignalingServer] Error adding ICE candidate: {e}")
                    except json.JSONDecodeError as e:
                        print(f"[SignalingServer] Error parsing peer message: {e}")

        except Exception as e:
            print(f"[SignalingServer] Error handling connection for client {client_id}: {e}")
        finally:
            self.connections.remove(websocket)
            print(f"[SignalingServer] Client {client_id} disconnected")
            print(f"[SignalingServer] Remaining active connections: {len(self.connections)}")

    def on_negotiation_needed(self, element):
        """Handle WebRTC negotiation-needed signal."""
        if self.offer_in_progress:
            print("[SignalingServer] Offer already in progress, skipping...")
            return

        print("[SignalingServer] Negotiation needed, creating offer...")
        try:
            self.offer_in_progress = True
            promise = Gst.Promise.new_with_change_func(self.on_offer_created, None)
            element.emit('create-offer', None, promise)
            print("[SignalingServer] Offer creation initiated")
        except Exception as e:
            print(f"[SignalingServer] Error initiating offer creation: {e}")
            self.offer_in_progress = False

    def on_offer_created(self, promise, user_data):
        """Handle offer creation."""
        print("[SignalingServer] Processing created offer...")
        try:
            reply = promise.get_reply()
            offer = reply.get_value('offer')
            print("[SignalingServer] Got offer from promise")

            promise = Gst.Promise.new()
            self.webrtcbin.emit('set-local-description', offer, promise)
            promise.interrupt()
            print("[SignalingServer] Local description set")

            # Convert offer to string and store it
            self.current_offer = offer.sdp.as_text()
            print("[SignalingServer] Converted and stored SDP")

            # Send offer to all connected clients
            message = json.dumps({
                'offer': {
                    'type': 'offer',
                    'sdp': self.current_offer
                }
            })
            message = f"ROOM_PEER_MSG server {message}"

            client_count = len(self.connections)
            print(f"[SignalingServer] Sending offer to {client_count} connected clients")

            if client_count == 0:
                print("[SignalingServer] Warning: No clients connected to send offer to")
                return

            # Send to all clients using the event loop
            if hasattr(self, 'loop') and not self.loop.is_closed():
                for ws in self.connections:
                    try:
                        future = asyncio.run_coroutine_threadsafe(ws.send(message), self.loop)
                        future.result()  # Wait for the send to complete
                        print(f"[SignalingServer] Offer sent to client {id(ws)}")
                    except Exception as e:
                        print(f"[SignalingServer] Error sending offer to client {id(ws)}: {e}")

        except Exception as e:
            print(f"[SignalingServer] Error in offer creation: {e}")
        finally:
            self.offer_in_progress = False

    def on_ice_candidate(self, element, mlineindex, candidate):
        """Handle new ICE candidate."""
        try:
            # Send ICE candidate to all connected clients
            message = json.dumps({
                'iceCandidate': {
                    'candidate': candidate,
                    'sdpMLineIndex': mlineindex
                }
            })
            message = f"ROOM_PEER_MSG server {message}"

            if hasattr(self, 'loop') and not self.loop.is_closed():
                for ws in self.connections:
                    try:
                        future = asyncio.run_coroutine_threadsafe(ws.send(message), self.loop)
                        future.result()  # Wait for the send to complete
                        print(f"[SignalingServer] ICE candidate sent to client {id(ws)}")
                    except Exception as e:
                        print(f"[SignalingServer] Error sending ICE candidate to client {id(ws)}: {e}")

        except Exception as e:
            print(f"Error sending ICE candidate: {e}")