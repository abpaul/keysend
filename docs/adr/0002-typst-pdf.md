# The application makes the Export with the Typst library

The Export is a PDF that the Typst library renders inside the application. We chose this because it needs no external program on a shared server, and a spike measured 21 ms and 47 MB for a Typst PDF of 16 pages against 1.4 s and 323 MB for a LibreOffice conversion.

## Considered Options

- LibreOffice converts a document to PDF: it adds about 800 MB to the image and uses seven times the memory for each Export.
- A headless browser prints an HTML page to PDF: a heavy process.

## Consequences

- The Export has its own layout in Typst, separate from the HTML page. A change to what the Ad Account page shows needs a change in both.
- The fonts are embedded in the application, so that the PDF is the same on every machine.
- At most two renders run at the same time.
