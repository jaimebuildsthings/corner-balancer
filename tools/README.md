# Tools

## Plot a test run

1. Capture a run: start the monitor with a log, then `stream on` at the console.
   ```sh
   espflash monitor | tee run.log
   ```
2. Open [`plot.html`](plot.html) in a browser (double-click it; no server needed) and drop `run.log` on it.

You get kg and temperature over time per channel, and the residuals from the last `cal fit` in the log. Drag across a chart to zoom; the stats table (mean, std dev, spread, drift) follows the zoomed range. Zoom into an empty-pad stretch to read zero noise, or a held load to read creep.

Nothing leaves your machine. [uPlot](https://github.com/leeoniya/uPlot) 1.6.32 (MIT) is vendored in `vendor/uplot/`.
