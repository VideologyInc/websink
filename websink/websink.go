// SPDX-FileCopyrightText: 2023 The Pion community <https://pion.ly>
// SPDX-License-Identifier: MIT

//go:build !js
// +build !js

// gstreamer-send is a simple application that shows how to send video to your browser using Pion WebRTC and GStreamer.
package main

import (
	"embed"
	"encoding/base64"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"

	"github.com/go-gst/go-gst/gst"
	"github.com/go-gst/go-gst/gst/app"
	"github.com/pion/webrtc/v4"
	"github.com/pion/webrtc/v4/pkg/media"

	_ "github.com/motemen/go-loghttp/global" // Just this line!
)

// SessionRequest represents the JSON structure for session requests
type SessionRequest struct {
	Offer string `json:"offer"`
}

//go:embed static/*
var static_files embed.FS

func main() {
	audioSrc := flag.String("audio-src", "audiotestsrc", "GStreamer audio src")
	videoSrc := flag.String("video-src", "videotestsrc", "GStreamer video src")
	flag.Parse()

	// Initialize GStreamer
	gst.Init(nil)

	// Prepare the configuration
	config := webrtc.Configuration{}

	// Create a new RTCPeerConnection
	peerConnection, err := webrtc.NewPeerConnection(config)
	if err != nil {
		panic(err)
	}

	// Set the handler for ICE connection state
	// This will notify you when the peer has connected/disconnected
	peerConnection.OnICEConnectionStateChange(func(connectionState webrtc.ICEConnectionState) {
		fmt.Printf("Connection State has changed %s \n", connectionState.String())
	})

	// Create a audio track
	audioTrack, err := webrtc.NewTrackLocalStaticSample(webrtc.RTPCodecCapability{MimeType: "audio/opus"}, "audio", "pion1")
	if err != nil {
		panic(err)
	}
	_, err = peerConnection.AddTrack(audioTrack)
	if err != nil {
		panic(err)
	}

	// Create a video track
	firstVideoTrack, err := webrtc.NewTrackLocalStaticSample(webrtc.RTPCodecCapability{MimeType: "video/h264"}, "video", "pion2")
	if err != nil {
		panic(err)
	}
	_, err = peerConnection.AddTrack(firstVideoTrack)
	if err != nil {
		panic(err)
	}

	// Start pushing buffers on these tracks
	go pipelineForCodec("opus", []*webrtc.TrackLocalStaticSample{audioTrack}, *audioSrc)
	go pipelineForCodec("h264", []*webrtc.TrackLocalStaticSample{firstVideoTrack}, *videoSrc)

	// Set up HTTP handlers
	m := http.NewServeMux()
	fileserver := http.FileServer(http.FS(static_files))
	m.Handle("GET /static/", fileserver)
	m.Handle("GET /favicon.ico", fileserver)
	m.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		http.ServeFile(w, r, "static/index.html")
	})
	m.HandleFunc("/favicon.ico", func(w http.ResponseWriter, r *http.Request) {
		http.ServeFile(w, r, "static/favicon.ico")
	})
	m.HandleFunc("/api/session", handleSession(peerConnection))

	println("Server started at http://localhost:8082")

	http.ListenAndServe(":8082", m)
	// Block forever
	select {}
}

// handleSession creates a handler for the /api/session endpoint
func handleSession(peerConnection *webrtc.PeerConnection) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}

		// Read the request body
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Error reading request body", http.StatusBadRequest)
			return
		}

		// Parse the JSON request
		var sessionReq SessionRequest
		if err := json.Unmarshal(body, &sessionReq); err != nil {
			http.Error(w, "Error parsing JSON", http.StatusBadRequest)
			return
		}

		// Decode the offer
		offer := webrtc.SessionDescription{}
		decode(sessionReq.Offer, &offer)

		// Set the remote SessionDescription
		if err := peerConnection.SetRemoteDescription(offer); err != nil {
			http.Error(w, "Error setting remote description: "+err.Error(), http.StatusInternalServerError)
			return
		}

		// Create an answer
		answer, err := peerConnection.CreateAnswer(nil)
		if err != nil {
			http.Error(w, "Error creating answer: "+err.Error(), http.StatusInternalServerError)
			return
		}

		// Sets the LocalDescription, and starts our UDP listeners
		if err = peerConnection.SetLocalDescription(answer); err != nil {
			http.Error(w, "Error setting local description: "+err.Error(), http.StatusInternalServerError)
			return
		}

		// Wait for ICE gathering to complete
		gatherComplete := webrtc.GatheringCompletePromise(peerConnection)
		<-gatherComplete

		// Return the answer as JSON
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]string{
			"answer": encode(peerConnection.LocalDescription()),
		})
	}
}

// Create the appropriate GStreamer pipeline depending on what codec we are working with
func pipelineForCodec(codecName string, tracks []*webrtc.TrackLocalStaticSample, pipelineSrc string) {
	pipelineStr := "appsink name=appsink"
	switch codecName {
	case "vp8":
		pipelineStr = pipelineSrc + " ! vp8enc error-resilient=partitions keyframe-max-dist=10 auto-alt-ref=true cpu-used=5 deadline=1 ! " + pipelineStr
	case "vp9":
		pipelineStr = pipelineSrc + " ! vp9enc ! " + pipelineStr
	case "h264":
		pipelineStr = pipelineSrc + " ! video/x-raw,format=I420 ! x264enc speed-preset=ultrafast tune=zerolatency key-int-max=20 ! video/x-h264,stream-format=byte-stream ! " + pipelineStr
	case "opus":
		// pipelineStr = pipelineSrc + " ! avenc_opus ! " + pipelineStr
		pipelineStr = pipelineSrc + " ! opusenc ! " + pipelineStr
	case "pcmu":
		pipelineStr = pipelineSrc + " ! audio/x-raw, rate=8000 ! mulawenc ! " + pipelineStr
	case "pcma":
		pipelineStr = pipelineSrc + " ! audio/x-raw, rate=8000 ! alawenc ! " + pipelineStr
	default:
		panic("Unhandled codec " + codecName) //nolint
	}

	pipeline, err := gst.NewPipelineFromString(pipelineStr)
	if err != nil {
		panic(err)
	}

	if err = pipeline.SetState(gst.StatePlaying); err != nil {
		panic(err)
	}

	appSink, err := pipeline.GetElementByName("appsink")
	if err != nil {
		panic(err)
	}

	app.SinkFromElement(appSink).SetCallbacks(&app.SinkCallbacks{
		NewSampleFunc: func(sink *app.Sink) gst.FlowReturn {
			sample := sink.PullSample()
			if sample == nil {
				return gst.FlowEOS
			}

			buffer := sample.GetBuffer()
			if buffer == nil {
				return gst.FlowError
			}

			samples := buffer.Map(gst.MapRead).Bytes()
			defer buffer.Unmap()

			for _, t := range tracks {
				if err := t.WriteSample(media.Sample{Data: samples, Duration: *buffer.Duration().AsDuration()}); err != nil {
					panic(err) //nolint
				}
			}

			return gst.FlowOK
		},
	})
}

// Read from stdin until we get a newline
// func readUntilNewline() (in string) {
// 	var err error

// 	r := bufio.NewReader(os.Stdin)
// 	for {
// 		in, err = r.ReadString('\n')
// 		if err != nil && !errors.Is(err, io.EOF) {
// 			panic(err)
// 		}

// 		if in = strings.TrimSpace(in); len(in) > 0 {
// 			break
// 		}
// 	}

// 	fmt.Println("")
// 	return
// }

// JSON encode + base64 a SessionDescription
func encode(obj *webrtc.SessionDescription) string {
	b, err := json.Marshal(obj)
	if err != nil {
		panic(err)
	}

	return base64.StdEncoding.EncodeToString(b)
}

// Decode a base64 and unmarshal JSON into a SessionDescription
func decode(in string, obj *webrtc.SessionDescription) {
	b, err := base64.StdEncoding.DecodeString(in)
	if err != nil {
		panic(err)
	}

	if err = json.Unmarshal(b, obj); err != nil {
		panic(err)
	}
}
