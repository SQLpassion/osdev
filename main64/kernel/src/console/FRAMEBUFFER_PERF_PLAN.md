# Umsetzungsplan: Framebuffer-Konsole Performance (VBE/GOP)

> **Ziel:** Das „Zeitlupen"-Scrollen der Framebuffer-Konsole auf physischer Hardware beseitigen.
> **Status:** offen / noch nicht begonnen.
> **Branch-Basis bei Erstellung:** `feature/uefi`.
> Dieses Dokument ist self-contained und kann in einer anderen Session/Maschine ohne weiteren Kontext abgearbeitet werden.

---

## 0. Kontext & relevante Dateien

Die Konsole rendert Text in einen linearen Framebuffer (BIOS/VBE oder UEFI/GOP) mit einem 8×16-Bitmap-Font.

| Datei | Rolle |
|-------|-------|
| `main64/kernel/src/console/framebuffer.rs` | **Hauptdatei** – `FramebufferConsole`, Rendering, Scroll, Redraw |
| `main64/kernel/src/console/interface.rs` | Trait `KernelConsole`, globaler `GLOBAL_CONSOLE` (SpinLock), `with_console`, `init` |
| `main64/kernel/src/console/mod.rs` | Modul-Re-Exports |
| `main64/kernel/src/console/font_basic.rs` | `FONT_BASIC` (8×16, als `FONT_8X16` importiert) |
| `main64/kernel/src/boot_info.rs` | `FramebufferInfo { base_address: u64, size: usize, width: u32, height: u32, pixels_per_scanline: u32, pixel_format: PixelFormat }` |
| `main64/kernel/src/main.rs` | `map_framebuffer()` (Zeile ~353), PAT-Setup, `booted_via_framebuffer()` |
| `main64/kernel/src/memory/vmm/mapping.rs` | `configure_wc_mapping()` (~777), `map_virtual_to_physical_wc()` (~262), `invlpg()` |
| `main64/kernel/src/arch/msr.rs` | `rdmsr(u32) -> u64`, `wrmsr(u32, u64)` (beide `unsafe`) |

### Wichtige Eckdaten der aktuellen Implementierung
- `cells: Vec<u16>` ist ein **Shadow-Buffer** im RAM, je Zelle `(attr << 8) | char` (VGA-Attribut: `(bg << 4) | fg`).
- Geometrie: `cols = width/8`, `rows = height/16`.
- `pixels_per_scanline` ist der **Stride** (kann > `width` sein, Padding). VRAM-Offset = `y * pixels_per_scanline + x`.
- `pixel_format`: `Bgr` (Default) oder `Rgb`; Farbumrechnung in `color_to_rgb()`.
- Scroll setzt `deferred_redraw = true`; `flush_redraw()` ruft `redraw_from_cells()` auf.
- `redraw_from_cells()` rastert **den gesamten Bildschirm** neu und kopiert ihn komplett ins VRAM.

---

## 1. Problemdiagnose (warum es langsam ist)

**Hauptengpass:** `redraw_from_cells()` (`framebuffer.rs:313`) wird bei **jedem** Scroll (jedes `\n` über die letzte Zeile) ausgelöst und rastert **alle** Zellen neu + kopiert den **kompletten** Framebuffer ins VRAM.

Bei 1920×1080 / 32 bpp (≈240×67 Zeichen):
- ~2 Mio. Pixel-Berechnungen pro Scroll (mit Bit-Test-Branch je Pixel).
- ~8 MB VRAM-Upload pro Scroll.
- Einmal **pro Print-Aufruf, der scrollt** → z. B. 100-Zeilen-Ausgabe ≈ 100 Voll-Rasterungen + ~830 MB MMIO-Traffic.

Beim Scrollen sind aber 66 von 67 Zeilen nur **verschoben** und müssten nicht neu gerastert werden.

**Zweites Risiko (potenziell katastrophal):** Falls Write-Combining auf realer Hardware nicht wirklich aktiv ist, ist der Framebuffer effektiv **uncached (UC)** → jeder Pixel-Write ist ein synchroner Bus-Zyklus.

---

## 2. Arbeitsschritte

Reihenfolge bewusst gewählt: **erst messen (Schritt A), dann WC absichern (B), dann Algorithmus (C/D)**. A+B können das Problem evtl. schon allein lösen; falls nicht, liefern C/D den strukturellen Gewinn.

> Pro Schritt jeweils einen eigenen Commit. Nach jedem Schritt auf echter Hardware gegentesten (QEMU zeigt das Problem **nicht** zuverlässig, da QEMU-VRAM schnell ist).

---

### Schritt A — Messen / Instrumentierung (Diagnose, kein Funktionsumbau)

**Ziel:** Quantifizieren, wo die Zeit verloren geht, bevor optimiert wird.

1. Eine grobe Zeitmessung um `fill_physical_screen()` und um `redraw_from_cells()` legen (z. B. über vorhandenen Time-/TSC-Treiber unter `drivers/time/`; vorher prüfen, was verfügbar ist — `rdtsc` ist die einfachste Quelle).
2. Einmalig beim Boot messen und via `debugln!`/serielle Konsole ausgeben:
   - Dauer eines Full-Screen-Fills (= reine VRAM-Schreibbandbreite).
   - Dauer eines `redraw_from_cells()` (= Rasterung + Upload).
3. **Interpretation:**
   - Full-Fill extrem langsam (zig ms für 8 MB) → **UC-Verdacht** → Schritt B ist die Lösung.
   - Full-Fill ok, aber `redraw_from_cells` dominiert → Schritt C/D ist die Lösung.

**Akzeptanz:** Zahlen liegen vor; Entscheidung dokumentiert, ob B und/oder C/D nötig sind.
**Hinweis:** Diese Instrumentierung ist temporär — nach Abschluss wieder entfernen (oder hinter ein Debug-Flag legen).

---

### Schritt B — Write-Combining wirklich absichern

**Hintergrund:** `main.rs` setzt PAT1 = WC (MSR `0x277`, Bits 8..15 auf `0x01`) und `configure_wc_mapping()` setzt `PWT=1` je Page (+ `invlpg`). Das ist konzeptionell korrekt, **aber:**

1. **Fehlender Cache-Flush.** Nach Änderung des Cache-Typs einer bereits (potenziell) gecachten Region verlangt das Intel SDM (Vol. 3, „Programming the PAT") ein definiertes Vorgehen: betroffene Pages aus dem Cache entfernen. In `configure_wc_mapping()` (`mapping.rs:777`) wird pro Page nur `invlpg` gemacht, **kein `WBINVD`**.
   - **Maßnahme:** Nach Abschluss der WC-Konfiguration des Framebuffer-Bereichs einmal `WBINVD` ausführen.
   - In `arch/` existiert noch **kein** `wbinvd`-Helper → kleinen `unsafe`-Wrapper ergänzen:
     ```
     // in arch/ (z. B. neue fn in einem passenden Modul):
     // unsafe { core::arch::asm!("wbinvd", options(nostack)); }
     ```
   - Aufruf nach `configure_wc_mapping(...)` bzw. nach der Map-Schleife in `map_framebuffer()` (`main.rs:393`).
2. **Reihenfolge prüfen:** PAT-MSR-Write (`main.rs:370`) erfolgt **vor** den Mappings — das ist ok, solange danach TLB invalidiert wird (passiert via `invlpg`). Sicherstellen, dass kein Mapping mit `PWT=1` existiert, **bevor** PAT1 = WC programmiert ist (sonst kurzzeitig WT-Semantik). Aktuell unkritisch, da beides in `map_framebuffer()` nacheinander läuft.
3. **Verifikation:** Schritt-A-Messung nach der Änderung wiederholen. Ein WC-Framebuffer sollte einen Full-Fill um ~1–2 Größenordnungen schneller schreiben als UC.

**Wichtig — write-combining Schreibmuster:** WC-Puffer flushen am besten bei **sequEntiellen, ausgerichteten** Schreibzugriffen. Die bestehenden `copy_nonoverlapping`-Scanline-Kopien sind dafür gut geeignet; **Per-Pixel-`write_volatile`** (in `write_pixel`, `draw_cursor`, `erase_cursor`) ist für WC ungünstig (kleine, verstreute Writes) → in Schritt D adressiert.

**Akzeptanz:** Full-Fill-Zeit aus Schritt A deutlich reduziert; auf echter HW sichtbar flüssigeres Vollbild-Clear.

---

### Schritt C — Scrollen per Block-Move statt Voll-Rasterung (größter algorithmischer Gewinn)

**Kernidee:** Beim Scrollen wird der Bildinhalt nur **verschoben**. Statt alle Glyphen neu zu rastern, die bereits gerenderten Pixel um eine Textzeile (16 Scanlines) nach oben kopieren und **nur die neue unterste Zeile** neu rastern.

**Empfohlene Umsetzung mit RAM-Backbuffer (Shadow-Framebuffer):**

1. **Neues Feld** in `FramebufferConsole`: ein Pixel-Backbuffer im normalen (gecachten) RAM, z. B. `back: Vec<u32>` der Größe `pixels_per_scanline * height` (oder kompakter `render_width * height`). Einmal in `new()` allozieren (siehe auch Schritt E: Allokationen aus dem Hot-Path entfernen).
2. **Alle Render-Pfade schreiben zuerst in den RAM-Backbuffer**, dann wird (nur der geänderte Bereich) ins VRAM kopiert. Betroffen: `draw_char_pixel`, `redraw_from_cells`, `fill_physical_screen`, Cursor.
3. **`scroll()` umbauen:**
   - `cells` weiterhin nach oben verschieben (wie bisher).
   - Zusätzlich im **Backbuffer** `memmove` um `16 * stride` Pixel nach oben (RAM→RAM, schnell). In Rust: `copy_within` auf dem Slice.
   - Unterste Textzeile (16 Scanlines) im Backbuffer mit Hintergrundfarbe füllen.
   - Statt Voll-Rasterung nur die neue (leere) Zeile ist bereits korrekt; falls die neue Zeile Inhalt bekommt, rastert der normale Zeichen-Pfad sie.
4. **VRAM-Upload:** Nach dem Scroll den Backbuffer ins VRAM kopieren — idealerweise nur den geänderten Bereich (siehe Schritt D, Dirty-Tracking). Als Zwischenschritt: ganzen Backbuffer kopieren (immer noch besser, da Rasterung entfällt) und in Schritt D auf Dirty-Region einschränken.

**Resultat:** Pro Scroll entfallen ~2 Mio. branchende Pixel-Berechnungen; ersetzt durch einen RAM-`copy_within` + Rasterung von nur einer Zeile (≈240 Glyphen).

**Akzeptanz:** `redraw_from_cells()` wird im Scroll-Pfad **nicht** mehr aufgerufen (oder ist drastisch günstiger). Scrollen auf echter HW flüssig.

---

### Schritt D — Dirty-Region-Tracking + Redraw-Bündelung (VRAM-Bandbreite senken)

**Ziel:** Den verbleibenden teuren Teil — den VRAM-Upload — minimieren.

1. **Dirty-Range je Frame** mitführen: kleinste/größte geänderte Scanline (`dirty_y_min`, `dirty_y_max`) bzw. Rechteck. Jeder Schreibzugriff in den Backbuffer erweitert die Dirty-Range.
2. **Flush kopiert nur die Dirty-Scanlines** vom Backbuffer ins VRAM (sequenzielle `copy_nonoverlapping`-Blöcke, WC-freundlich), danach Dirty-Range zurücksetzen.
3. **Redraw über Print-Aufrufe hinweg bündeln:**
   - Aktuell flusht `print_char()` (`framebuffer.rs:493`) nach **jedem** Zeichen, und `deferred_redraw` bündelt nur innerhalb eines `write_str`.
   - Umstellen auf: Schreibzugriffe markieren nur Dirty-Region; **ein** Flush an einem definierten Punkt (z. B. am Ende von `write_str`/`print_str`, vor Tastatureingabe-Warten, oder per explizitem `flush()`).
   - Achtung Korrektheit: Sicherstellen, dass vor jeder Eingabe-/Warteoperation und vor Panic-Ausgaben geflusht wird, damit nichts „hängen" bleibt. Panic-Pfad (`panic.rs`) und die VGA-Panic-Schreiber prüfen.
4. **Cursor** in das Dirty-/Flush-Modell integrieren (statt sofortiger Per-Pixel-VRAM-Writes), siehe Schritt E.

**Akzeptanz:** Eine n-zeilige Ausgabe erzeugt nicht mehr n Voll-Uploads; nur tatsächlich geänderte Scanlines gehen ins VRAM.

---

### Schritt E — Feinschliff

1. **Allokationen aus dem Hot-Path entfernen:**
   - `redraw_from_cells()` (`:321`) und `fill_physical_screen()` (`:693`) allozieren je Aufruf einen `Vec`. → Backbuffer aus Schritt C wiederverwenden; keine Allokation im Render-/Scroll-Pfad.
2. **Cursor ohne Per-Pixel-`write_volatile`:**
   - `draw_cursor`/`erase_cursor` (`:269`, `:305`) schreiben pixelweise direkt ins VRAM. → In den Backbuffer schreiben + Dirty markieren; Cursor-Zelle aus `cells` rekonstruieren (Logik existiert in `erase_cursor`).
3. **Glyph-Rasterung optimieren (optional):**
   - Innerloop `(byte & (1 << (7 - col)))` + `color_to_rgb` pro Zelle. → `fg_rgb`/`bg_rgb` aus Schleifen ziehen (teils vorhanden) und Font-Byte→8-Pixel branchfrei expandieren, ggf. Lookup-Tabelle „Byte (256) → Pixelmaske".
4. **`pixels_per_scanline` vs. `width`** konsequent beachten: Stride für Offsets ist `pixels_per_scanline`, gefüllt wird nur bis `width`. Beim Backbuffer entscheiden, ob er Stride-breit (einfacher 1:1-Upload) oder `width`-breit (kompakter, aber Upload muss Padding überspringen) angelegt wird. **Empfehlung:** Backbuffer Stride-breit (`pixels_per_scanline * height`) → Upload ist ein einziger zusammenhängender `copy_nonoverlapping` pro Dirty-Block.

**Akzeptanz:** Keine Heap-Allokationen mehr im Render-Hot-Path; Cursor erzeugt keine verstreuten VRAM-Writes.

---

## 3. Test- & Verifikationsstrategie

- **QEMU** zeigt das Performance-Problem nicht (schnelles emuliertes VRAM) — dient nur als **Korrektheits-Regressionstest** (Text/Scroll/Cursor/Box korrekt?).
- **Echte Hardware** (BIOS/VBE **und** UEFI/GOP separat, da unterschiedliche Mapping-Pfade) ist die maßgebliche Performance-Messung.
- Reproduzierbarer Stress-Test: ein langes, vielzeiliges Scroll-Szenario (z. B. großes Verzeichnis listen oder Schleife mit vielen `\n`), Zeit messen / visuell beurteilen.
- Nach **jedem** Schritt: Build (`cargo`-Build des Kernels gemäß Projekt-Setup; Build-Kommando vorab im Repo verifizieren, z. B. Makefile/`x.py`/`cargo` unter `main64/`), Integrationstests unter `tests/` laufen lassen.
- **Korrektheit besonders prüfen:** Scroll-Inhalt korrekt verschoben, unterste Zeile leer/korrekt, Cursor an richtiger Position, `clear()`/`draw_box()`/`fill_rect()`/`blit_framebuffer()` weiterhin korrekt, Panic-Ausgabe sichtbar.

---

## 4. Risiken & Hinweise

- **`WBINVD`** ist teuer (flusht den gesamten Cache), aber wird hier nur **einmal** beim Boot/FB-Setup benötigt → unkritisch. Nicht in einen Hot-Path legen.
- **Backbuffer-Speicher:** `pixels_per_scanline * height * 4` Bytes (bei 1920×1080 ≈ 8 MB). Sicherstellen, dass der Kernel-Heap das hergibt (`memory/heap/`); ggf. Größe loggen. Bei knappem Heap Fallback auf bisherigen Pfad behalten.
- **SpinLock-Haltedauer:** `with_console` hält den globalen Lock mit deaktivierten Interrupts. Lange Voll-Uploads im Lock erhöhen Latenz → ein weiterer Grund, per Dirty-Region nur Nötiges zu kopieren.
- **Reihenfolge nicht umkehren:** Ohne wirksames WC (Schritt B) bringt der Backbuffer (C) zwar weniger Rasterung, aber der finale VRAM-Upload bleibt langsam.
- **`PixelFormat`** Bgr/Rgb-Unterscheidung in `color_to_rgb` beibehalten; Backbuffer speichert bereits formatkorrekte `u32`-Pixel (wie bisher), Upload ist dann ein reiner `memcpy`.

---

## 5. Definition of Done

- [ ] Schritt A: Mess-Zahlen erhoben und Entscheidung dokumentiert (Instrumentierung danach entfernt/abschaltbar).
- [ ] Schritt B: `WBINVD`-Helper ergänzt, nach WC-Konfiguration aufgerufen; WC auf echter HW verifiziert.
- [ ] Schritt C: RAM-Backbuffer eingeführt; `scroll()` nutzt `copy_within` + Rasterung nur der neuen Zeile; keine Voll-Rasterung im Scroll-Pfad.
- [ ] Schritt D: Dirty-Region-Upload + gebündelter Flush; Korrektheit bei Eingabe/Panic sichergestellt.
- [ ] Schritt E: keine Hot-Path-Allokationen; Cursor über Backbuffer; optionale Glyph-Optimierung.
- [ ] Scrollen auf physischer Hardware (BIOS/VBE **und** UEFI/GOP) flüssig; Integrationstests grün.
