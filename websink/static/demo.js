// SPDX-FileCopyrightText: 2023 The Pion community <https://pion.ly>
// SPDX-License-Identifier: MIT

/* eslint-env browser */

let pc = new RTCPeerConnection({
  iceServers: [
    {
      urls: 'stun:stun.l.google.com:19302'
    }
  ]
})

pc.ontrack = function (event) {
  console.log("New track received: " + event.track.kind)
  var el = document.createElement(event.track.kind)
  el.srcObject = event.streams[0]
  el.autoplay = true
  el.controls = true
  el.playsInline = true  // Important for mobile devices

  // Start muted to bypass browser autoplay restrictions
  el.muted = true

  // Add loadedmetadata event listener to ensure media is ready
  el.addEventListener('loadedmetadata', function() {
    // Ensure playback starts
    el.play().catch(e => {
      console.warn(`Autoplay failed: ${e.message}. User interaction may be required.`);
    });
  });

  document.getElementById('remoteVideos').appendChild(el)
}

let iceTimout = null

function sendOffer() {
  console.log('sending offer...')

  // Display the offer in the textarea for reference
  document.getElementById('localSessionDescription').value = JSON.stringify(pc.localDescription)

  console.log('ICE gathering complete, sending offer to server...')

  // Send the offer to the server as JSON directly
  fetch('/api/session', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      offer: pc.localDescription
    }),
  })
  .then(response => {
    if (!response.ok) {
      throw new Error(`Server responded with ${response.status}: ${response.statusText}`)
    }
    return response.json()
  })
  .then(data => {
    console.log('Received answer from server')

    // Set the remote description with the answer from the server
    pc.setRemoteDescription(data.answer)

    // Display the answer in the textarea for reference
    document.getElementById('remoteSessionDescription').value = JSON.stringify(data.answer)

    console.log('Connection established automatically')
  })
  .catch(error => {
    console.error(`Error: ${error.message}`);
    alert(`Failed to establish connection: ${error.message}`);
  });
}

pc.oniceconnectionstatechange = e => {
  console.log("ICE connection state changed: " + pc.iceConnectionState)

  // When connection is established, try to play all media elements
  if (pc.iceConnectionState === 'connected' || pc.iceConnectionState === 'completed') {
    // Find all video and audio elements and try to play them
    document.querySelectorAll('video, audio').forEach(el => {
      el.play().catch(e => {
        console.warn(`Autoplay failed: ${e.message}`);
      });
    });
  }
}

pc.onicecandidate = event => {
  if (event.candidate === null) {
    // ICE gathering is complete, we can now send the offer to the server
    console.log('ICE gathering complete')
  } else {
    console.log("New ICE candidate received")
    // fire after 150ms of no new candidates
    if (iceTimout) {
      clearTimeout(iceTimout)
    }
    iceTimout = setTimeout(sendOffer, 150)
  }
}

// Offer to receive 1 audio, and 1 video track
pc.addTransceiver('audio', {'direction': 'sendrecv'})
pc.addTransceiver('video', {'direction': 'sendrecv'})
pc.createOffer()
   .then(offer => {
       console.log("Offer created");
       return pc.setLocalDescription(offer);
   })
   .catch(error => {
       console.error(`Error: ${error.message}`);
   });

// Keep the startSession function for manual fallback
window.startSession = () => {
  let sd = document.getElementById('remoteSessionDescription').value
  if (sd === '') {
    return alert('Session Description must not be empty')
  }

  try {
    // Parse the JSON directly
    pc.setRemoteDescription(JSON.parse(sd))
    console.log('Connection established manually')
  } catch (e) {
    console.error(`Error: ${e.message}`);
  }
}
