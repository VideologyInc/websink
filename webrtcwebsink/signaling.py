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
    def __init__(self, webrtcbin_factory, host='0.0.0.0', port=8081):
        self.webrtcbin_factory = webrtcbin_factory
        self.host = host
        self.port = port
        self.server = None
        self.clients = {}  # Map of client_id -> {websocket, webrtcbin}
        self.running = False
        self.offer_in_progress = False
        self.loop = None

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
        self.loop = loop
        asyncio.set_event_loop(loop)
        try:
            loop.run_until_complete(self.run())
        finally:
            self.loop = None
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
        if self.clients:
            print(f"[SignalingServer] Closing {len(self.clients)} active connections...")
            close_tasks = [client['websocket'].close() for client in self.clients.values()]
            await asyncio.gather(*close_tasks, return_exceptions=True)
            self.clients.clear()
            print("[SignalingServer] All connections closed")

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

            # Create a new WebRTCbin for this client
            webrtcbin = self.webrtcbin_factory()
            if not webrtcbin:
                print(f"[SignalingServer] Failed to create WebRTCbin for client {client_id}")
                await websocket.close(code=1011, reason='internal server error')
                return

            # Connect to webrtcbin signals for this client
            webrtcbin.connect('on-negotiation-needed',
                             lambda element: self.on_negotiation_needed(element, client_id))
            webrtcbin.connect('on-ice-candidate',
                             lambda element, mlineindex, candidate:
                             self.on_ice_candidate(element, mlineindex, candidate, client_id))
            # Add client to our map
            self.clients[client_id] = {
                'websocket': websocket,
                'webrtcbin': webrtcbin
            }
            print(f"[SignalingServer] Client {client_id} connected successfully")
            print(f"[SignalingServer] Total active connections: {len(self.clients)}")

            # Trigger negotiation for this client
            print(f"[SignalingServer] Triggering negotiation for client {client_id}")
            self.on_negotiation_needed(webrtcbin, client_id)

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

                                # Use the client's WebRTCbin
                                promise = Gst.Promise.new()
                                webrtcbin.emit('set-remote-description', answer, promise)
                                promise.interrupt()
                                print(f"[SignalingServer] Successfully set remote description for client {client_id}")
                            except Exception as e:
                                print(f"[SignalingServer] Error setting remote description: {e}")

                        elif 'iceCandidate' in data:
                            print(f"[SignalingServer] Processing ICE candidate from client {client_id}")
                            try:
                                webrtcbin.emit('add-ice-candidate',
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
            # Clean up client resources
            if client_id in self.clients:
                del self.clients[client_id]
            print(f"[SignalingServer] Client {client_id} disconnected")
            print(f"[SignalingServer] Remaining active connections: {len(self.clients)}")

    def on_negotiation_needed(self, element, client_id=None):
        """Handle WebRTC negotiation-needed signal."""
        # Skip if client doesn't exist
        if client_id is None or client_id not in self.clients:
            print(f"[SignalingServer] Client {client_id} not found, skipping negotiation")
            return

        print(f"[SignalingServer] Negotiation needed for client {client_id}, creating offer...")
        try:
            # Get the client's WebRTCbin
            webrtcbin = self.clients[client_id]['webrtcbin'] if client_id else element

            # Create offer
            promise = Gst.Promise.new_with_change_func(
                lambda promise, data: self.on_offer_created(promise, client_id),
                None
            )
            element.emit('create-offer', None, promise)
            print(f"[SignalingServer] Offer creation initiated for client {client_id}")
        except Exception as e:
            print(f"[SignalingServer] Error initiating offer creation for client {client_id}: {e}")

    def on_offer_created(self, promise, client_id):
        """Handle offer creation."""
        print(f"[SignalingServer] Processing created offer for client {client_id}...")
        try:
            # Skip if client doesn't exist
            if client_id not in self.clients:
                print(f"[SignalingServer] Client {client_id} not found, skipping offer")
                return

            # Get the client's WebRTCbin and websocket
            client = self.clients[client_id]
            webrtcbin = client['webrtcbin']
            websocket = client['websocket']

            reply = promise.get_reply()
            offer = reply.get_value('offer')
            print(f"[SignalingServer] Got offer from promise for client {client_id}")

            promise = Gst.Promise.new()
            webrtcbin.emit('set-local-description', offer, promise)
            promise.interrupt()
            print(f"[SignalingServer] Local description set for client {client_id}")

            # Convert offer to string and store it
            offer_sdp = offer.sdp.as_text()
            print(f"[SignalingServer] Converted SDP for client {client_id}")

            # Send offer to this client
            message = json.dumps({
                'offer': {
                    'type': 'offer',
                    'sdp': offer_sdp
                }
            })
            message = f"ROOM_PEER_MSG server {message}"
            print(f"[SignalingServer] Sending offer to client {client_id}")

            # Send the offer to the client
            asyncio.run_coroutine_threadsafe(
                self._send_message_to_client(websocket, message, f"Offer sent to client {client_id}"),
                self.loop
            )

        except Exception as e:
            print(f"[SignalingServer] Error in offer creation for client {client_id}: {e}")

    def on_ice_candidate(self, element, mlineindex, candidate, client_id=None):
        """Handle new ICE candidate."""
        try:
            # Skip if client doesn't exist or loop is not available
            if client_id is None or client_id not in self.clients or not self.loop:
                print(f"[SignalingServer] Client {client_id} not found, skipping ICE candidate")
                return

            # Get the client's websocket
            websocket = self.clients[client_id]['websocket'] if client_id else None

            # Create ICE candidate message
            message = json.dumps({
                'iceCandidate': {
                    'candidate': candidate,
                    'sdpMLineIndex': mlineindex
                }
            })
            message = f"ROOM_PEER_MSG server {message}"
            print(f"[SignalingServer] Sending ICE candidate to client {client_id}")

            # Send the ICE candidate to the client
            asyncio.run_coroutine_threadsafe(
                self._send_message_to_client(websocket, message, f"ICE candidate sent to client {client_id}"),
                self.loop
            )

        except Exception as e:
            print(f"Error sending ICE candidate to client {client_id}: {e}")

    async def _send_message_to_client(self, websocket, message, success_message):
        """Helper method to send a message to a client and handle errors."""
        try:
            if not websocket:
                print("[SignalingServer] No websocket provided, cannot send message")
                return
            await websocket.send(message)
            print(f"[SignalingServer] {success_message}")
        except Exception as e:
            print(f"[SignalingServer] Error sending message: {e}")