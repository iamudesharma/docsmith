This page documents the draw.io file format: the `.drawio` XML structure, page compression, the cell layer, user objects, how diagrams are embedded in PNG, SVG, HTML and PDF exports, and the custom library format. It is for developers who read or generate diagram files outside the editor. The in-memory objects behind the XML are described in [Graph model](Graph-Model).

## File types

| Extension | Content |
|---|---|
| `.drawio`, `.xml` | XML with an `<mxfile>` root (older files may have a bare `<mxGraphModel>` root). |
| `.drawio.svg` | SVG image; the `<mxfile>` XML is in the `content` attribute of the root `<svg>`. |
| `.drawio.png` | PNG image; the `<mxfile>` XML is in a `tEXt` chunk with the keyword `mxfile`. |
| `.html` | HTML page that renders the diagram with the viewer; the XML is in the `data-mxgraph` attribute. |
| `.drawio.pdf` | PDF with the XML in the document metadata or as an attachment. |
| `.xml` with `<mxlibrary>` | Custom shape library, see [Libraries](#custom-libraries). |

Exports that include a copy of the diagram are named `name.drawio.png`, `name.drawio.svg` or `name.drawio.pdf` by the editor.

## Structure

```
<mxfile host="...">                     file: metadata only
  <diagram id="..." name="Page-1">      one per page
    <mxGraphModel dx="..." grid="1" ...> page settings
      <root>
        <mxCell id="0"/>                root cell
        <mxCell id="1" parent="0"/>     default layer
        ...cells...
      </root>
    </mxGraphModel>
  </diagram>
</mxfile>
```

Instead of the `<mxGraphModel>` child, a `<diagram>` may contain the same XML as compressed text (see [Compression](#compression)). Both forms can appear in one file.

Files are read by `EditorUi.prototype.setFileData` ([`diagramly/EditorUi.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/diagramly/EditorUi.js)) with `Editor.extractGraphModel` and `Editor.parseDiagramNode` ([`diagramly/Editor.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/diagramly/Editor.js)), and written by `EditorUi.prototype.getXmlFileData` and `EditorUi.prototype.createFileData` (`diagramly/EditorUi.js`).

## The mxfile element

`createFileData` rewrites the file-level metadata on every save. Other attributes are file properties that are kept as they are.

| Attribute | Written by | Meaning |
|---|---|---|
| `host` | every save | Host name of the editor (`window.location.hostname`), `Electron` for the desktop app. |
| `pages` | every save, if more than one page | Number of pages. |
| `compressed` | File > Properties | `true` / `false`: stores pages compressed or not, overriding the default (see [When files are compressed](#when-files-are-compressed)). |
| `locked` | File > Properties (`DrawioFile.prototype.setLocked`) | `true` disables editing until it is unlocked. |
| `vars` | File > Properties > Edit Data | JSON object of file variables, available as `%name%` placeholders. |
| `scale`, `border` | File > Properties (`.drawio.png` / `.drawio.svg`) | Zoom and border used when the image is regenerated on save. |
| `embedFonts` | File > Properties (`.drawio.svg`) | `false` disables embedding fonts in the SVG. |
| `linkTarget` | not written by the editor | Default target for links in exports (`graph.defaultExportLinkTarget`). |

Older files also carry `agent`, `version`, `modified`, `etag`, `type`, `userAgent` and `editor`. Current versions do not write them; the save code removes most of them, and readers should ignore all of them. With the `compact` configuration option, `host` and these legacy attributes are not written.

## The diagram element

| Attribute | Meaning |
|---|---|
| `id` | Page ID, unique in the file (a duplicate is a load error). Page links use it: `data:page/id,<id>`. |
| `name` | Page name shown in the tab. |
| `viewBox` | Optional initial view, `x y width height [scale]` (`DiagramPage.prototype.getViewBox`). |

## Compression

A compressed page is the `<mxGraphModel>` XML, URI-encoded, deflated without a zlib header (raw DEFLATE) and Base64-encoded. The core of `Graph.compress` and `Graph.decompress` in [`Graph.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/grapheditor/Graph.js):

```javascript
// Graph.compress(data)
var tmp = pako.deflateRaw(encodeURIComponent(data));
return btoa(Graph.arrayBufferToString(new Uint8Array(tmp)));

// Graph.decompress(data)
var tmp = Graph.stringToArrayBuffer(atob(data));
var inflated = decodeURIComponent(pako.inflateRaw(tmp, {to: 'string'}));
```

`encodeURIComponent` runs **before** the deflate, so the inflated bytes are ASCII. `Graph.decompress` also removes control characters (`Graph.zapGremlins`) from the result. The editor bundles [pako](https://github.com/nodeca/pako) (`js/deflate/pako.min.js`).

### JavaScript

In current browsers and in Node.js 22, without dependencies (`CompressionStream` with `deflate-raw`):

```javascript
async function decompressDiagram(text)
{
	var bytes = Uint8Array.from(atob(text.trim()), function(c) { return c.charCodeAt(0); });
	var stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('deflate-raw'));

	return decodeURIComponent(await new Response(stream).text());
}

async function compressDiagram(xml)
{
	var stream = new Blob([encodeURIComponent(xml)]).stream().pipeThrough(new CompressionStream('deflate-raw'));
	var bytes = new Uint8Array(await new Response(stream).arrayBuffer());
	var binary = '';

	for (var i = 0; i < bytes.length; i++)
	{
		binary += String.fromCharCode(bytes[i]);
	}

	return btoa(binary);
}
```

With pako:

```javascript
function decompressDiagram(text)
{
	var bytes = Uint8Array.from(atob(text.trim()), function(c) { return c.charCodeAt(0); });

	return decodeURIComponent(pako.inflateRaw(bytes, {to: 'string'}));
}

function compressDiagram(xml)
{
	var bytes = pako.deflateRaw(encodeURIComponent(xml));
	var binary = '';

	for (var i = 0; i < bytes.length; i++)
	{
		binary += String.fromCharCode(bytes[i]);
	}

	return btoa(binary);
}
```

### Python

```python
import base64
import urllib.parse
import xml.etree.ElementTree as ET
import zlib


def decompress_diagram(text):
    """Returns the <mxGraphModel> XML of a compressed <diagram> text node."""
    data = base64.b64decode(text)
    xml = zlib.decompress(data, -15)  # negative wbits: raw DEFLATE, no header
    return urllib.parse.unquote(xml.decode('utf-8'))


def compress_diagram(xml):
    """Inverse of decompress_diagram, same as Graph.compress."""
    # The characters that JavaScript's encodeURIComponent leaves unescaped
    encoded = urllib.parse.quote(xml, safe="!'()*-._~")
    deflate = zlib.compressobj(9, zlib.DEFLATED, -15)
    data = deflate.compress(encoded.encode('ascii')) + deflate.flush()
    return base64.b64encode(data).decode('ascii')


def read_pages(xml):
    """Yields (name, mxGraphModel XML) for every page in the XML of a file."""
    root = ET.fromstring(xml)
    if root.tag == 'mxGraphModel':
        yield None, xml
    for diagram in root.iter('diagram'):
        model = diagram.find('mxGraphModel')
        if model is not None:
            yield diagram.get('name'), ET.tostring(model, encoding='unicode')
        elif diagram.text and diagram.text.strip():
            yield diagram.get('name'), decompress_diagram(diagram.text.strip())


with open('diagram.drawio', encoding='utf-8') as f:
    for name, model_xml in read_pages(f.read()):
        print(name, len(model_xml))
```

These snippets round-trip with `Graph.compress` / `Graph.decompress` in both directions. To inspect a file by hand, use Extras > Edit Diagram in the editor, or the [Text Tools page](https://jgraph.github.io/drawio-tools/tools/text-tools.html) from [jgraph/drawio-tools](https://github.com/jgraph/drawio-tools).

### When files are compressed

New files are written uncompressed. `DrawioFile.prototype.isCompressed` decides per file:

1. If `<mxfile>` has a `compressed` attribute, `compressed != "false"` wins (set it with File > Properties > Compressed).
2. Otherwise pages are compressed only if the `compressXml` [configuration](https://www.drawio.com/docs/reference/configure-diagram-editor/) option is `true` (`Editor.defaultCompressed`, default `false`).

A compressed file opened without a `compressed` attribute is therefore saved uncompressed. Uncompressed files diff well in version control; keep them that way for files you generate or review.

## The mxGraphModel element

The attributes hold the page settings. They are written by `Editor.prototype.getGraphXml` ([`grapheditor/Editor.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/grapheditor/Editor.js) and [`diagramly/Editor.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/diagramly/Editor.js)) for the current page and `Graph.prototype.saveViewState` ([`Pages.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/diagramly/Pages.js)) for the others, and read by `Editor.prototype.readGraphState` and `Graph.prototype.createViewState`. All are optional.

| Attribute | Values | Meaning |
|---|---|---|
| `dx`, `dy` | number | View translation (scroll position) when saved. Only written when not 0. |
| `grid` | `0` / `1` | Grid visible. The `grid` URL parameter overrides it. |
| `gridSize` | number | Grid size, default 10. |
| `guides` | `0` / `1` | Alignment guides when moving cells. |
| `tooltips` | `0` / `1` | Show tooltips. |
| `connect` | `0` / `1` | Connection Points: new connections can be drawn from shapes (`graph.setConnectable`). |
| `arrows` | `0` / `1` | Connection Arrows shown when hovering over a shape (`graph.connectionArrowsEnabled`). |
| `fold` | `0` / `1` | Show the collapse/expand icons of containers. |
| `page` | `0` / `1` | Page view (page outlines). The `pv` URL parameter overrides it. |
| `pageScale` | number | Page scale, default 1. |
| `pageWidth`, `pageHeight` | number | Page size in pixels, for example 850 x 1100 for US Letter, 827 x 1169 for A4. |
| `background` | colour or `none` | Page background colour. |
| `backgroundImage` | JSON | Background image, for example `{"src":"...","width":...,"height":...}`. |
| `math` | `0` / `1` | Typeset LaTeX and AsciiMath in labels. |
| `shadow` | `0` / `1` | Shadow on all shapes. |
| `adaptiveColors` | `auto`, `simple`, `none` | Dark mode colour handling for this page; omitted means the default (see [Styles](Styles#colours-and-dark-mode)). |
| `extFonts` | `name^url\|name^url` | Web fonts used on the page. |
| `style` | name | Stylesheet theme; only written when the graph uses a theme other than the default. |

The codec ignores these attributes (`mxModelCodec.decodeAttributes`); the editor reads them itself, so a model without any of them loads with defaults.

## Cells

`<root>` holds every cell of the page as a flat list. The tree is defined by the `parent` attribute, not by nesting. Cell `0` (the root, no parent) and cell `1` (the default layer, `parent="0"`) are conventional: the editor creates them for new pages, and a well-formed file has exactly one cell without a parent.

Attributes of `<mxCell>` (`mxCellCodec`, [`mxgraph/src/io/`](https://github.com/jgraph/drawio/tree/dev/src/main/webapp/mxgraph/src/io)); defaults are not written:

| Attribute | Meaning |
|---|---|
| `id` | Unique within the page. |
| `value` | Label (XML-escaped). With `html=1` in the style it is HTML. |
| `style` | Style string, see [Styles](Styles). |
| `vertex="1"` / `edge="1"` | Shape or connector. Layers and the root have neither. |
| `parent` | ID of the containing cell: a layer, group, container or (for edge labels) an edge. |
| `source`, `target` | Edges: IDs of the connected cells. |
| `connectable="0"` | Edges cannot connect to the cell (edge labels, groups). |
| `visible="0"` | Hidden cell or layer. |
| `collapsed="1"` | Collapsed container. |

The geometry is a child element:

```xml
<!-- vertex: bounds relative to the parent's origin -->
<mxGeometry x="40" y="40" width="120" height="60" as="geometry"/>

<!-- edge: always relative="1"; x/y place the edge's own label -->
<mxGeometry relative="1" as="geometry">
  <mxPoint x="40" y="150" as="sourcePoint"/>    <!-- end point if there is no source -->
  <mxPoint x="600" y="122" as="targetPoint"/>   <!-- end point if there is no target -->
  <Array as="points">                           <!-- waypoints, parent coordinates -->
    <mxPoint x="320" y="150"/>
  </Array>
</mxGeometry>

<!-- edge label (child vertex of an edge): x from -1 (source) to 1 (target),
     y = orthogonal distance, offset in pixels -->
<mxGeometry x="-0.19" y="2" relative="1" as="geometry">
  <mxPoint x="-10" y="6" as="offset"/>
</mxGeometry>

<!-- collapsed container: bounds to restore when expanded -->
<mxGeometry x="40" y="40" width="160" height="30" as="geometry">
  <mxRectangle x="40" y="40" width="300" height="200" as="alternateBounds"/>
</mxGeometry>
```

A vertex with `relative="1"` is positioned at a fraction (0..1) of its parent's size plus `offset`. The meaning of every field is in [Graph model](Graph-Model#mxgeometry).

Where an edge connects is defined by its style (`exitX`, `exitY`, `entryX`, `entryY`, ...), not by its geometry. With no such keys the edge floats around the terminal's perimeter.

## User objects

A cell with custom data has an XML element as its value. In the file the element wraps the `mxCell`, takes the cell's `id` and holds the label in its `label` attribute:

```xml
<UserObject label="web-01" owner="ops" ip="10.0.0.1" tooltip="Primary"
            link="https://example.com" placeholders="1" tags="prod linux" id="5">
  <mxCell style="rounded=1;whiteSpace=wrap;html=1;" vertex="1" parent="1">
    <mxGeometry x="40" y="200" width="120" height="60" as="geometry"/>
  </mxCell>
</UserObject>
```

The editor writes `UserObject` (`Graph.prototype.setAttributeForCell`) or `object` (Edit Data dialog); other element names are read and kept as they are. Attributes with a meaning (`Graph.prototype.builtInProperties` and the link handling):

| Attribute | Meaning |
|---|---|
| `label` | The label. |
| `tooltip` | Tooltip. Without it, the tooltip lists the other attributes. |
| `note` | Markdown note (Edit Note). |
| `link`, `linkTarget` | Link and its target window. `data:page/id,<id>` links to a page; `data:action/json,{...}` is a [custom link action](https://www.drawio.com/docs/reference/animation-actions-json/). `javascript:` links are removed when read. |
| `placeholders="1"` | Replace `%name%` in label and tooltip with attributes of the cell, its ancestors, the root cell's user object, and the file `vars`. |
| `placeholder` | Use the named attribute as the label. |
| `tags` | Space-separated tags. |
| `label_<lang>`, `tooltip_<lang>` | Translations, used when diagram translation is on (URL parameters `translate-diagram=1` and `diagram-language`). |

The root cell (`id="0"`) can also be a user object; its attributes are page-wide placeholder values, and the `animation` attribute holds the page's step animation. See [Placeholders](https://www.drawio.com/docs/manual/advanced/placeholders/).

## Style compression

With the `compressStyles` configuration option, `mxModelCodec` stores a data-URI image or inline `stencil(...)` shape that is used more than once in a `<defs>` element before `<root>`, and the styles reference it:

```xml
<mxGraphModel>
  <defs>
    <def data="data:image/png,iVBORw0KGgo..."/>
  </defs>
  <root>
    ...
    <mxCell id="7" style="shape=image;image=def(0);" vertex="1" parent="1">
```

`def(n)` is the index of the `<def>` element; only `image` and `shape` values are replaced. Readers that do not support this see blank shapes. See [Style compression](https://www.drawio.com/docs/reference/style-compression/) for the versions that read it.

## A minimal file

```xml
<mxfile>
  <diagram id="p1" name="Page-1">
    <mxGraphModel>
      <root>
        <mxCell id="0"/>
        <mxCell id="1" parent="0"/>
        <mxCell id="2" value="Hello" style="rounded=1;whiteSpace=wrap;html=1;" vertex="1" parent="1">
          <mxGeometry x="40" y="40" width="120" height="60" as="geometry"/>
        </mxCell>
      </root>
    </mxGraphModel>
  </diagram>
</mxfile>
```

A bare `<mxGraphModel>` is also accepted when opened or imported; the editor wraps it in a page.

## An annotated example

```xml
<mxfile host="app.diagrams.net" pages="2" vars='{"project":"Atlas"}'>
  <diagram id="overview" name="Overview">
    <mxGraphModel dx="0" dy="0" grid="1" gridSize="10" guides="1" tooltips="1" connect="1"
        arrows="1" fold="1" page="1" pageScale="1" pageWidth="827" pageHeight="1169"
        math="0" shadow="0">
      <root>
        <mxCell id="0"/>
        <mxCell id="1" parent="0"/>
        <!-- second layer, hidden -->
        <mxCell id="notes" value="Notes" parent="0" visible="0"/>

        <!-- container with a child; the child's x/y are relative to the container -->
        <mxCell id="c1" value="Cluster" style="swimlane;whiteSpace=wrap;html=1;" vertex="1" parent="1">
          <mxGeometry x="40" y="40" width="200" height="140" as="geometry"/>
        </mxCell>
        <UserObject id="s1" label="%project% web" placeholders="1" owner="ops">
          <mxCell style="rounded=1;whiteSpace=wrap;html=1;" vertex="1" parent="c1">
            <mxGeometry x="40" y="50" width="120" height="60" as="geometry"/>
          </mxCell>
        </UserObject>

        <mxCell id="db" value="Database" style="shape=cylinder3;whiteSpace=wrap;html=1;" vertex="1" parent="1">
          <mxGeometry x="360" y="80" width="80" height="80" as="geometry"/>
        </mxCell>

        <!-- edge in layer 1 between cells in different parents; fixed exit and entry points -->
        <mxCell id="e1" style="edgeStyle=orthogonalEdgeStyle;rounded=0;html=1;exitX=1;exitY=0.5;entryX=0;entryY=0.5;"
            edge="1" parent="1" source="s1" target="db">
          <mxGeometry relative="1" as="geometry"/>
        </mxCell>
        <!-- label of e1, placed 40% of the way from the centre to the target -->
        <mxCell id="e1-label" value="SQL" style="edgeLabel;html=1;" vertex="1" connectable="0" parent="e1">
          <mxGeometry x="0.4" relative="1" as="geometry">
            <mxPoint as="offset"/>
          </mxGeometry>
        </mxCell>

        <!-- dangling edge with a waypoint -->
        <mxCell id="e2" style="endArrow=classic;html=1;" edge="1" parent="1" source="db">
          <mxGeometry relative="1" as="geometry">
            <mxPoint x="560" y="40" as="targetPoint"/>
            <Array as="points">
              <mxPoint x="520" y="120"/>
            </Array>
          </mxGeometry>
        </mxCell>

        <mxCell id="n1" value="Review" style="shape=note;whiteSpace=wrap;html=1;" vertex="1" parent="notes">
          <mxGeometry x="300" y="220" width="100" height="60" as="geometry"/>
        </mxCell>
      </root>
    </mxGraphModel>
  </diagram>
  <diagram id="details" name="Details">
    <mxGraphModel>
      <root>
        <mxCell id="0"/>
        <mxCell id="1" parent="0"/>
        <UserObject id="back" label="Back to overview" link="data:page/id,overview">
          <mxCell style="text;html=1;" vertex="1" parent="1">
            <mxGeometry x="40" y="40" width="140" height="30" as="geometry"/>
          </mxCell>
        </UserObject>
      </root>
    </mxGraphModel>
  </diagram>
</mxfile>
```

## Diagrams embedded in exports

| Format | Where the XML is | Encoding | Reader in the code |
|---|---|---|---|
| PNG | `tEXt` chunk, keyword `mxfile`, placed before the first `IDAT` | `encodeURIComponent` of the `<mxfile>` XML | `Editor.extractGraphModelFromPng` ([`grapheditor/Editor.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/grapheditor/Editor.js)); writer `Editor.writeGraphModelToPng` |
| SVG | `content` attribute of the root `<svg>` | The `<mxfile>` XML as an attribute value (XML-escaped); older files may URI-encode it | `Editor.extractGraphModel`; writer `EditorUi.prototype.getEmbeddedSvg` |
| HTML | `data-mxgraph` attribute of `<div class="mxgraph">` | HTML-escaped JSON; the `xml` property holds the `<mxfile>` XML | `Editor.extractGraphModel`; writer `EditorUi.prototype.getHtml2` |
| PDF | `/Subject` metadata, or an embedded file of type `application/vnd.jgraph.mxfile` | Subject: URI-encoded XML; embedded file: the XML | `Editor.extractGraphModelFromPdf` ([`diagramly/Editor.js`](https://github.com/jgraph/drawio/blob/dev/src/main/webapp/js/diagramly/Editor.js)) |

Readers should also accept a `zTXt` chunk and the keyword `mxGraphModel` in PNG files: `Editor.extractGraphModelFromPng` handles both (`zTXt` is zlib-compressed, as the PNG specification requires). The pages inside the embedded `<mxfile>` can be compressed or not, as in a `.drawio` file. The editor only reads the embedded copy; the image is regenerated when an editable PNG or SVG is saved.

Python, for PNG and SVG:

```python
import struct
import urllib.parse
import xml.etree.ElementTree as ET
import zlib


def xml_from_png(path):
    """Returns the <mxfile> XML embedded in a .drawio.png, or None."""
    with open(path, 'rb') as f:
        data = f.read()
    if data[:8] != b'\x89PNG\r\n\x1a\n':
        raise ValueError('not a PNG file')
    pos = 8
    while pos < len(data):
        length, ctype = struct.unpack('>I4s', data[pos:pos + 8])
        chunk = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if ctype in (b'tEXt', b'zTXt'):
            keyword, _, text = chunk.partition(b'\0')
            if keyword in (b'mxfile', b'mxGraphModel'):
                if ctype == b'zTXt':
                    text = zlib.decompress(text[1:])  # first byte: compression method
                return urllib.parse.unquote(text.decode('latin-1'))
        elif ctype == b'IDAT':
            break
    return None


def xml_from_svg(path):
    """Returns the <mxfile> XML embedded in a .drawio.svg, or None."""
    content = ET.parse(path).getroot().get('content')
    if content and content.startswith('%'):
        content = urllib.parse.unquote(content)
    return content
```

Both return the `<mxfile>` XML, which `read_pages` (above) splits into pages: `read_pages(xml_from_png('diagram.drawio.png'))`.

## Custom libraries

A library is an `<mxlibrary>` element whose text is a JSON array, written by `EditorUi.prototype.createLibraryDataFromImages` and read by `EditorUi.prototype.loadLibrary`:

```xml
<mxlibrary title="My shapes">[
  {"xml": "&lt;mxGraphModel&gt;...&lt;/mxGraphModel&gt;", "w": 120, "h": 60, "title": "Box"},
  {"data": "data:image/png;base64,iVBORw0KGgo...", "w": 48, "h": 48, "title": "Logo", "aspect": "fixed"}
]</mxlibrary>
```

Each entry has `w` and `h` and either `xml` (an `<mxGraphModel>`, plain or compressed like a page) or `data` (an image data URI), plus optional `title`, `aspect` and `style`. The optional `title` and `tags` attributes of `<mxlibrary>` name the library and add search tags. Full reference: [Custom shape library format](https://www.drawio.com/docs/reference/format-custom-shape-library/). Libraries of icons and shapes are published in [jgraph/drawio-libs](https://github.com/jgraph/drawio-libs).

## Generating files

- Give every cell an ID that is unique in its page, and every page an ID that is unique in the file.
- Include cells `0` and `1`; give every other cell a `parent` that exists. A cell with a missing or unknown parent is moved into the first layer when the file is loaded (`mxModelCodec.adoptOrphanedCells`, with a console message).
- Set exactly one of `vertex="1"` or `edge="1"` on shapes and connectors, and give each an `<mxGeometry as="geometry">`; write `relative="1"` on edge geometries as the editor does.
- Give an edge a `source` and `target` that exist, or a `sourcePoint` / `targetPoint` for each missing end. A reference to an unknown ID is dropped (with a console error) and that end is left unconnected.
- Put an edge in the nearest common ancestor of its terminals, usually the layer, since its waypoints are relative to its parent.
- Escape `&`, `<`, `>` and `"` in attribute values. For HTML labels (`html=1`) the value is escaped HTML, for example `value="&lt;b&gt;Title&lt;/b&gt;"`.
- In style strings `;` and `=` separate entries, so values that contain them must be encoded; images use data URIs without `;base64` (see [Styles](Styles#syntax)).
- Write pages uncompressed (no `compressed` attribute needed); they are smaller to generate, easy to diff and read by every draw.io version.
- Leave out the page settings you do not need; defaults apply.
- To check a generated file, open it in the editor (Extras > Edit Diagram shows the parsed XML) or in the [viewer](Embed-Diagrams).

The [style reference](https://www.drawio.com/docs/reference/diagram-generation/style-reference/) lists shapes and style keys with examples for generated diagrams.

## See also

- [Graph model](Graph-Model): the objects behind the XML.
- [Styles](Styles): the style attribute.
- [Import and export](Import-and-Export): conversion to and from other formats.
- [Embedding](Embedding) and [Embed diagrams](Embed-Diagrams): loading files into the editor and the viewer.
- [Shapes and stencils](Shapes-and-Stencils): `shape=` values and stencil XML.
- [Security](Security): diagram files are untrusted input.
