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

  document.getElementById('remoteVideos').appendChild(el)
}

let iceTimout = null

function sendOffer() {
  console.log('sending offer...')
  const offerBase64 = btoa(JSON.stringify(pc.localDescription))
  document.getElementById('localSessionDescription').value = offerBase64

  console.log('ICE gathering complete, sending offer to server...')

  // Send the offer to the server
  fetch('/api/session', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ offer: offerBase64 }),
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
    const answerSDP = JSON.parse(atob(data.answer))
    pc.setRemoteDescription(answerSDP)

    // Display the answer in the textarea for reference
    document.getElementById('remoteSessionDescription').value = data.answer

    console.log('Connection established automatically')
  })
  .catch(error => {
    console.error(`Error: ${error.message}`);
    alert(`Failed to establish connection: ${error.message}`);
  });
}

pc.oniceconnectionstatechange = e => console.log("ICE connection state changed: " + pc.iceConnectionState)
pc.onicecandidate = event => {
  if (event.candidate === null) {
    // ICE gathering is complete, we can now send the offer to the server
    console.log('ICE gathering complete')
  } else {
    console.log("New ICE candidate received: " + JSON.stringify(event.candidate))
    // fire after 150ms of no new candidates
    if (iceTimout) {
      clearTimeout(iceTimout)
    }
    iceTimout = setTimeout(sendOffer, 150)
  }
}

// Offer to receive 1 audio, and 2 video tracks
pc.addTransceiver('audio', {'direction': 'sendrecv'})
pc.addTransceiver('video', {'direction': 'sendrecv'})
// pc.addTransceiver('video', {'direction': 'sendrecv'})
pc.createOffer()
   .then(offer => {
       console.log("Offer created: ");
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
    pc.setRemoteDescription(JSON.parse(atob(sd)))
    console.log('Connection established manually')
  } catch (e) {
    console.error(`Error: ${e.message}`);
  }
}
