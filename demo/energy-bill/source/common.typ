#let setup(body) = {
  set page(
    paper: "a4",
    margin: (x: 2cm, top: 2cm, bottom: 2.2cm),
    footer: align(center, text(8pt, fill: gray)[Sample document for the Pathfinder demo — fictional. Kestrelby Energy is an invented supplier.]),
  )
  set text(font: "Helvetica", size: 10.5pt)
  set par(justify: false)
  body
}
