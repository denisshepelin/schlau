# /// script
# dependencies = ["reportlab", "pillow"]
# ///
"""Generates the fixture PDFs (invoices, e-ticket, German statement, a scanned agenda on blue paper) and the
logo sketch into the directory given as the first argument."""
import sys, random
from pathlib import Path
from reportlab.lib.pagesizes import A4
from reportlab.pdfgen import canvas
from reportlab.lib.colors import HexColor, white, black
from PIL import Image, ImageDraw, ImageFilter

out = Path(sys.argv[1])
W, H = A4

def lines(c, x, y, rows, size=10, lead=15, font="Helvetica", color=black):
    c.setFont(font, size); c.setFillColor(color)
    for r in rows:
        c.drawString(x, y, r); y -= lead
    return y

def invoice(num, amount, date):
    c = canvas.Canvas(str(out / f"invoice-{num}.pdf"), pagesize=A4)
    c.setTitle(f"Invoice {num}")
    c.setFillColor(HexColor("#111111")); c.rect(0, H - 90, W, 90, fill=1, stroke=0)
    lines(c, 50, H - 55, ["Lumen Labs"], 22, font="Helvetica-Bold", color=white)
    y = lines(c, 50, H - 140, [f"Invoice {num}", f"Date: {date}", "Billed to: Alex Moreau", "Paid with Visa ending 4242 via Paystream"], 11)
    c.setStrokeColor(HexColor("#cccccc")); c.line(50, y - 10, W - 50, y - 10)
    y = lines(c, 50, y - 35, ["Description", "Lumen Pro, monthly plan", "Seats: 1", "Tax (0%)"], 11)
    lines(c, W - 160, y + 60, ["Amount", amount, "", "$0.00"], 11)
    lines(c, 50, y - 30, [f"Total paid: {amount}"], 14, font="Helvetica-Bold")
    lines(c, 50, 60, ["Lumen Labs Inc. · 500 Harbor Way · San Francisco, CA · billing@lumen.example"], 8)
    c.save()

for num, amount, date in [("INV-2026-4821", "$49.00", "2026-10-01"), ("INV-2026-4784", "$49.00", "2026-09-01"), ("INV-2026-4747", "$39.00", "2026-08-01")]:
    invoice(num, amount, date)

c = canvas.Canvas(str(out / "e-ticket-K7Q2ZP.pdf"), pagesize=A4)
c.setFillColor(HexColor("#0b3d2e")); c.rect(0, H - 110, W, 110, fill=1, stroke=0)
lines(c, 50, H - 60, ["Northwind Air", "Electronic ticket receipt"], 20, 26, "Helvetica-Bold", white)
lines(c, 50, H - 160, ["Booking reference: K7Q2ZP", "Passenger: MOREAU/ALEX MX", "Ticket number: 911-2400183375",
    "", "NW 214  San Francisco (SFO) → Lisbon (LIS)", "Mon 9 March 2026  departs 17:40  arrives 13:05+1", "Seat 23A · Economy · 1 checked bag 23 kg",
    "", "NW 215  Lisbon (LIS) → San Francisco (SFO)", "Sat 14 March 2026  departs 15:10  arrives 18:55", "Seat 21C · Economy"], 12, 18)
c.save()

c = canvas.Canvas(str(out / "Nebenkostenabrechnung-2025.pdf"), pagesize=A4)
lines(c, 50, H - 60, ["Hausverwaltung Krüger GmbH · Lindenstraße 12 · 10969 Berlin"], 9)
lines(c, 50, H - 110, ["Nebenkostenabrechnung 2025", ""], 18, font="Helvetica-Bold")
lines(c, 50, H - 150, ["Wohnung 3. OG links, Wrangelstraße 41, 10997 Berlin", "Abrechnungszeitraum: 01.01.2025 – 31.12.2025", "",
    "Heizkosten                     1.184,20 €", "Warmwasser                       412,75 €", "Grundsteuer                      298,10 €",
    "Müllabfuhr                       176,40 €", "Hausreinigung                    142,00 €", "Hausstrom                         64,35 €", "",
    "Summe Kosten                   2.277,80 €", "Vorauszahlungen               -2.100,00 €", "Nachzahlung                      177,80 €", "",
    "Bitte überweisen Sie den Betrag bis zum 15.12.2026.", "Die Wartung der Heizung findet am 18.11.2026 statt."], 11, 17, "Courier")
c.save()

# A scanned-looking agenda on blue paper: no text layer, so only the image can be indexed.
img = Image.new("RGB", (1240, 1754), (196, 218, 245))
d = ImageDraw.Draw(img)
try:
    from PIL import ImageFont
    big = ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial Bold.ttf", 54)
    mid = ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial.ttf", 34)
except Exception:
    big = mid = None
d.rectangle([0, 0, 1240, 160], fill=(38, 84, 160))
d.text((80, 50), "Q1 Planning: Draft Agenda", font=big, fill=(255, 255, 255))
rows = ["09:30  Welcome and goals", "10:00  Roadmap review", "11:15  Hiring plan", "12:30  Lunch", "13:30  Budget and headcount",
        "15:00  Offsite logistics (Lisbon)", "16:00  Open questions"]
for i, r in enumerate(rows):
    d.text((100, 260 + i * 90), r, font=mid, fill=(20, 40, 90))
for y in range(1000, 1600, 60):
    d.line([(100, y), (1140, y)], fill=(150, 180, 225), width=2)
img = img.rotate(0.8, fillcolor=(196, 218, 245)).filter(ImageFilter.GaussianBlur(0.6))
img.save(out / "planning-agenda.pdf", "PDF", resolution=150)

# Pencil logo sketches in blue on off-white paper.
random.seed(7)
img = Image.new("RGB", (1400, 900), (248, 246, 240))
d = ImageDraw.Draw(img)
blue = (40, 90, 200)
for k, (cx, cy) in enumerate([(260, 260), (700, 260), (1140, 260), (480, 650), (920, 650)]):
    for _ in range(6):
        r = 120 + random.randint(-6, 6)
        d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=blue, width=2)
    if k % 2 == 0:
        d.polygon([(cx - 60, cy + 50), (cx, cy - 70), (cx + 60, cy + 50)], outline=blue, width=4)
    else:
        d.rectangle([cx - 55, cy - 55, cx + 55, cy + 55], outline=blue, width=4)
        d.line([(cx - 55, cy + 55), (cx + 55, cy - 55)], fill=blue, width=4)
img.save(out / "logo-sketches-v2.png")
print("ok")
