// Canvas shim: tries @napi-rs/canvas first, falls back to mock
let napiCanvas;
try {
  napiCanvas = require('@napi-rs/canvas');
} catch (_) {
  napiCanvas = null;
}

if (napiCanvas) {
  // Use real canvas implementation
  module.exports = {
    createCanvas: (w, h) => {
      const canvas = napiCanvas.createCanvas(w, h);
      // Ensure toDataURL compatibility
      if (!canvas.toDataURL) {
        canvas.toDataURL = () => 'data:image/png;base64,';
      }
      return canvas;
    },
    loadImage: napiCanvas.loadImage,
    Canvas: napiCanvas.Canvas || function() {},
    Image: napiCanvas.Image || function() {},
    ImageData: napiCanvas.ImageData || function() {},
  };
} else {
  // Mock fallback for environments without native canvas
  module.exports = {
    createCanvas: (w, h) => ({
      width: w,
      height: h,
      getContext: () => ({
        fillRect: () => {},
        clearRect: () => {},
        drawImage: () => {},
        fillText: () => {},
        measureText: (t) => ({ width: t.length * 8 }),
        beginPath: () => {},
        closePath: () => {},
        moveTo: () => {},
        lineTo: () => {},
        stroke: () => {},
        fill: () => {},
        arc: () => {},
        save: () => {},
        restore: () => {},
        translate: () => {},
        rotate: () => {},
        scale: () => {},
        setTransform: () => {},
        createLinearGradient: () => ({ addColorStop: () => {} }),
        createRadialGradient: () => ({ addColorStop: () => {} }),
        putImageData: () => {},
        getImageData: () => ({ data: new Uint8ClampedArray(w * h * 4) }),
        createImageData: (w2, h2) => ({ data: new Uint8ClampedArray((w2||w) * (h2||h) * 4) }),
        canvas: { width: w, height: h, toDataURL: () => 'data:image/png;base64,' },
      }),
      toBuffer: () => Buffer.alloc(0),
      toDataURL: () => 'data:image/png;base64,',
    }),
    Image: class Image {
      constructor() { this.width = 0; this.height = 0; this.src = ''; }
    },
  };
}
