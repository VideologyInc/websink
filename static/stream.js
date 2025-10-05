/* eslint-env browser */

let pc = new RTCPeerConnection({
  iceServers: [
    {
      urls: 'stun:stun.l.google.com:19302'
    }
  ]
})

// Update connection status display
function showStatus(status, error = false) {
  const statusElement = document.getElementById('connectionStatus');
  statusElement.textContent = status;
  if (error) {
    statusElement.style.backgroundColor = 'rgba(255, 0, 0, 0.5)';
  } else {
    statusElement.style.backgroundColor = 'rgba(  0, 0, 0, 0.4)';
  }
}

function checkCodecCompatibility(receivedCodec) {
  const codecs = RTCRtpReceiver.getCapabilities('video').codecs;
  const codecString = codecs.map(c => c.mimeType).join(' ').toLowerCase();

  if (!codecString.includes(receivedCodec.toLowerCase())) {
    showStatus(`❌ This browser does not support ${receivedCodec.toUpperCase()} video codec. Try Safari for H.265 support.`, true);
    return false;
  }
  return true;
}

let videoElements = [];

pc.ontrack = function (event) {
  console.log('=== ontrack event fired ===');
  console.log('Track:', event.track.kind, event.track.id, event.track.label);
  console.log('Streams count:', event.streams.length);
  event.streams.forEach((stream, idx) => {
    console.log(`  Stream ${idx} id: ${stream.id}, tracks: ${stream.getTracks().length}`);
  });

  if (event.track.kind === 'video') {
    console.log('Processing video track:', event.track.id);

    const receiver = event.receiver;
    if (receiver && receiver.getParameters) {
      const params = receiver.getParameters();
      if (params.codecs && params.codecs.length > 0) {
        console.log('Negotiated codec:', params.codecs[0].mimeType);
      }
    }

    var videoElement = document.createElement('video');
    videoElement.srcObject = event.streams[0];
    videoElement.autoplay = true;
    videoElement.playsInline = true;
    videoElement.muted = true;
    videoElement.className = 'stream-video';

    videoElement.addEventListener('loadedmetadata', function() {
      videoElement.play().catch(e => {
        console.warn(`Autoplay failed: ${e.message}. User interaction may be required.`);
      });
    });

    const videoContainer = document.getElementById('videoContainer');
    videoContainer.appendChild(videoElement);
    videoElements.push(videoElement);

    console.log(`Total video tracks: ${videoElements.length}`);
    showStatus(`Playing ${videoElements.length} stream(s)`);
  }
}

let iceTimout = null

function sendOffer() {
  (async () => {
    try {
      console.log('Sending offer...');

      const res = await fetch('/api/session', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ offer: pc.localDescription }),
      });

      const contentType = res.headers.get('content-type') || '';
      const text = await res.text();

      if (!res.ok) {
        console.error(`❌ Offer declined: ${res.status} ${text}`);
        showStatus(`Connection failed: ${text || res.statusText}`, true);
        return;
      }

      const data = contentType.includes('application/json') && text ? JSON.parse(text) : {};
      console.log('Received answer from server');

      if (data.tracks && data.tracks.length > 0) {
        console.log(`Server is sending ${data.tracks.length} track(s):`);
        data.tracks.forEach((track, idx) => {
          console.log(`  Track ${idx}: ${track.codec} (${track.kind})`);
          if (!checkCodecCompatibility(track.codec)) return;
        });
        showStatus(`Receiving: ${data.tracks.length} stream(s)`);
      }

      await pc.setRemoteDescription(data.answer);
    } catch (err) {
      console.error(`❌ Connection failed: ${err.message}`);
      showStatus(`Connection failed: ${err.message}`, true);
    }
  })();
}

// Function to close the connection gracefully
function closeConnection() {
  console.log('Closing connection...');

  // Close the RTCPeerConnection
  // This will send the appropriate signals to the server
  if (pc && pc.connectionState !== 'closed') {
    // Close the peer connection
    pc.close();
    showStatus('Disconnected');
    console.log('Connection closed');
  }
}

pc.oniceconnectionstatechange = e => {
  const state = pc.iceConnectionState;
  console.log(`Connection: ${state}`);

  // When connection is established, try to play all video elements
  if (state === 'connected' || state === 'completed') {
    showStatus('Connected');
    // Find all video elements and try to play them
    document.querySelectorAll('video').forEach(el => {
      el.play().catch(e => {
        console.warn(`Autoplay failed: ${e.message}`);
      });
      showStatus('Playing');
    });
  }
}

pc.onicecandidate = event => {
  if (event.candidate === null) {
    // ICE gathering is complete, we can now send the offer to the server
    console.log('ICE gathering complete');
  } else {
    // fire after 150ms of no new candidates
    if (iceTimout) {
      clearTimeout(iceTimout)
    }
    iceTimout = setTimeout(sendOffer, 150)
  }
}

// Initialize connection
console.log('Starting connection...');

// Create offer without adding transceivers - let WebRTC negotiate based on server's tracks
pc.createOffer({ offerToReceiveAudio: true, offerToReceiveVideo: true })
   .then(offer => {
       console.log('Offer created');
       return pc.setLocalDescription(offer);
   })
   .catch(error => {
       console.log('Error creating offer');
       console.error(`Error: ${error.message}`);
   });

// Add event listener for page unload
window.addEventListener('beforeunload', function(event) {
  closeConnection();
});

// Add event listener for page visibility change
// document.addEventListener('visibilitychange', function() {
//   if (document.visibilityState === 'hidden') {
//     // Page is hidden (tab switched, minimized, etc.)
//     // This can help in some cases where beforeunload doesn't fire
//     closeConnection();
//   }
// });
