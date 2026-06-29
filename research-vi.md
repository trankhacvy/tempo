# Tempo `program/` — Giải thích sâu, đơn giản

Tài liệu này giải thích chương trình on-chain trên Solana nằm trong thư mục
`program/`. Nó được viết bằng **tiếng Việt đơn giản** để dễ theo dõi. Mục tiêu:
sau khi đọc xong, bạn hiểu **mọi tính năng** trong chương trình và **cách chương
trình xử lý từng tính năng**.

Chúng ta chỉ nói về `program/` (hợp đồng thông minh). Chúng ta không nói về phần
code Rust off-chain, client TypeScript, hay thư mục tài liệu.

> Lưu ý về thuật ngữ: nhiều từ chuyên ngành (perp, taker, maker, tick, oracle,
> funding...) được giữ nguyên tiếng Anh vì đây là từ chuẩn trong ngành DeFi và
> dịch ra sẽ khó hiểu hơn. Lần đầu xuất hiện sẽ có giải thích.

---

## Mục lục

1. [Bức tranh tổng thể: Tempo là gì và giải quyết vấn đề gì](#1-bức-tranh-tổng-thể)
2. [Ý tưởng cốt lõi: phiên đấu giá theo lô (batch auction)](#2-ý-tưởng-cốt-lõi)
3. [Mẹo histogram (các "hộp thư")](#3-mẹo-histogram)
4. [Giao thức clearing ba pha (trái tim của hệ thống)](#4-giao-thức-clearing-ba-pha)
5. [Phiên đấu giá kép (bên maker + bên taker)](#5-phiên-đấu-giá-kép)
6. [Phân bổ tại tick biên: chia khớp lệnh công bằng](#6-phân-bổ-tại-tick-biên)
7. [Vòng đời phiên đấu giá (máy trạng thái)](#7-vòng-đời-phiên-đấu-giá)
8. [Cách tổ chức code](#8-cách-tổ-chức-code)
9. [Mô hình tài khoản (tất cả cấu trúc dữ liệu)](#9-mô-hình-tài-khoản)
10. [Toàn bộ tập lệnh (instruction set)](#10-toàn-bộ-tập-lệnh)
11. [Luồng tiền: collateral, vault, nạp, rút](#11-luồng-tiền)
12. [Vị thế (position), funding, và PnL](#12-vị-thế-funding-và-pnl)
13. [Giá mark và oracle](#13-giá-mark-và-oracle)
14. [Thanh lý (liquidation)](#14-thanh-lý)
15. [Các tính năng gia cố rủi ro](#15-các-tính-năng-gia-cố-rủi-ro)
16. [Cross-margin (ký quỹ chéo)](#16-cross-margin)
17. [Maker quotes (thanh khoản tham số hóa)](#17-maker-quotes)
18. [Nâng cấp tài khoản (migration)](#18-nâng-cấp-tài-khoản)
19. [Các đặc tính an toàn (vì sao nó an toàn)](#19-các-đặc-tính-an-toàn)
20. [Chi tiết cấp thấp: bố cục zero-copy và sự kiện](#20-chi-tiết-cấp-thấp)
21. [Những phần còn thiếu (không phải lỗi)](#21-những-phần-còn-thiếu)

---

## 1. Bức tranh tổng thể

**Tempo là một sàn giao dịch phi tập trung (DEX) cho perpetuals** chạy trực tiếp
trên Solana. "Perpetual future" ("perp") là một hợp đồng cho phép bạn đặt cược
giá của một tài sản (ví dụ SOL) tăng hay giảm, có dùng đòn bẩy, và **không có ngày
đáo hạn**.

**Vấn đề mà Tempo khắc phục:** Hầu hết các sàn khớp lệnh **liên tục** — ngay khi
một lệnh đến là khớp ngay. Nghe có vẻ tốt, nhưng nó tạo ra một cuộc đua: **bot
nhanh nhất sẽ thắng**. Bot nhanh chen lên trước người dùng bình thường và rút giá
trị ra (gọi là **MEV** — "maximal extractable value", giá trị có thể trích xuất
tối đa). Tốc độ trở nên quan trọng hơn việc có giá tốt.

**Câu trả lời của Tempo: Dual Flow Batch Auction (DFBA) — phiên đấu giá theo lô,
hai dòng.** Thay vì khớp từng lệnh một, Tempo:

1. **Thu thập** tất cả lệnh trong một khoảng thời gian ngắn (cửa sổ).
2. Khi cửa sổ đóng, nó **khớp tất cả cùng lúc tại MỘT mức giá duy nhất**.

Mọi người trong cùng một lô đều nhận **cùng một mức giá**. Nhanh hơn 1 mili-giây
không cho bạn lợi thế gì, vì lệnh của bạn vẫn vào cùng lô với mọi người. Điều này
loại bỏ cuộc đua tốc độ và hầu hết MEV.

> Hãy hình dung như một buổi đấu giá ở trường. Thay vì bán mỗi món cho người hô
> trước, cô giáo thu hết các phiếu trả giá bí mật, rồi tìm một mức giá công bằng
> để khớp thị trường. Hô nhanh không quan trọng.

**Phần cốt lõi** là **bộ máy clearing** (toán học của phiên đấu giá). Bên trên đó,
chương trình thêm **luồng tiền** thật (collateral, nạp, rút), **quản lý rủi ro**
(funding, thanh lý, bảo hiểm), và **tính năng nâng cao** (cross-margin, maker
quotes).

---

## 2. Ý tưởng cốt lõi

Một **phiên đấu giá theo lô giá đồng nhất** hoạt động như sau:

- Người mua nói: "Tôi sẽ mua tối đa X tại giá P hoặc thấp hơn."
- Người bán nói: "Tôi sẽ bán tối đa Y tại giá P hoặc cao hơn."
- Sàn tìm **một mức giá** mà lượng người muốn mua bằng lượng người muốn bán. Mức
  giá đó là **giá clearing** (giá khớp).
- Mọi người giao dịch tại mức giá clearing duy nhất đó.

Phần khó trên blockchain: một sàn bình thường giữ toàn bộ **sổ lệnh** (order book —
danh sách tất cả lệnh) trong bộ nhớ và sắp xếp nó. Trên Solana, bộ nhớ và năng lực
tính toán ("compute units", CU) cho mỗi giao dịch **rất hạn chế**. Nếu có hàng
nghìn lệnh, bạn không thể nạp và sắp xếp tất cả trong một giao dịch.

Tempo giải quyết bằng một cấu trúc dữ liệu thông minh: **histogram giá**.

---

## 3. Mẹo histogram

### Phát hiện toán học then chốt

Bạn **không** cần toàn bộ sổ lệnh để tìm giá clearing. Bạn chỉ cần **tổng tích lũy
của cầu và cung tại mỗi mức giá**. Giá clearing có thể khôi phục được **chỉ từ các
tổng**.

Nên thay vì lưu mọi lệnh, Tempo lưu một **histogram** (biểu đồ cột) trên các **tick
giá**.

- Một **tick** là một bước giá nhỏ. Thị trường có số tick cố định (`num_ticks`, tối
  đa 256), và mỗi tick cách nhau `tick_size`.
- Với mỗi tick, histogram giữ vài bộ đếm `u64`: "lượng muốn mua ở đây" và "lượng
  muốn bán ở đây".

Nhóm phát triển gọi các bộ đếm này là **"các hộp thư" (mailboxes)**. Mỗi lệnh thả
khối lượng của nó vào đúng hộp thư (rổ giá), rồi biến mất khỏi phép toán. Chúng ta
không bao giờ cần tất cả lệnh trong bộ nhớ cùng lúc.

### Vì sao điều này mạnh mẽ

Chi phí clearing là **O(số tick)**, chứ không phải O(số lệnh). Dù có 10 lệnh hay
10.000 lệnh, histogram vẫn cùng kích thước cố định (nó chỉ phụ thuộc vào số tick
giá). Điều này làm cho clearing **rẻ và dự đoán được** trên Solana.

Histogram nằm trong tài khoản `AuctionHistogram`. Kích thước của nó là
`header + 4 vùng × num_ticks × 8 byte` (4 vùng được giải thích ở
[mục 5](#5-phiên-đấu-giá-kép)).

### Vì sao "gập" (folding) là bí quyết của sự an toàn

Cộng khối lượng của một lệnh vào hộp thư chỉ là **phép cộng số nguyên**:

```
hộp_thư[tick] = hộp_thư[tick] + khối_lượng_lệnh
```

Phép cộng có **tính giao hoán**: `a + b = b + a`. Thứ tự cộng **không** làm thay
đổi tổng cuối. Đây là đặc tính an toàn quan trọng nhất của Tempo, và chúng ta sẽ
quay lại nhiều lần. Nó nghĩa là: **dù ai xử lý các lệnh, theo thứ tự nào, histogram
cuối cùng vẫn giống hệt nhau.** Kẻ xấu không thể thao túng giá bằng cách chọn một
thứ tự khôn lỏi.

Hàm gập là `fold(...)` trong `state/histogram.rs`, và nó dùng **phép cộng có kiểm
tra (checked addition)** nên không bao giờ tràn số một cách âm thầm.

---

## 4. Giao thức clearing ba pha

Đây là **trái tim của Tempo**. Clearing được chia thành ba loại giao dịch rẻ tiền,
**không cần quyền (permissionless)**. "Không cần quyền" nghĩa là **bất kỳ ai** cũng
có thể gửi chúng — bạn không cần là một bên vận hành được tin cậy đặc biệt. Người
gửi các giao dịch này gọi là **cranker** (người "quay tay máy").

Ba pha là: **ACCUMULATE (tích lũy) → DISCOVER (khám phá) → SETTLE (thanh toán)**.

### Pha 1 — ACCUMULATE (`process_chunk`)

**Mục tiêu:** gập các lệnh đang chờ (resting) vào histogram.

- Một cranker gọi `process_chunk` với chỉ số bắt đầu và số lượng tối đa. Chương
  trình xử lý một **lát giới hạn** của order slab (ví dụ lệnh 0 đến 50). Nhiều giao
  dịch như vậy cùng nhau bao phủ tất cả lệnh. Vì thế nó "theo chunk (khúc)" — không
  một giao dịch nào phải làm tất cả.
- Với mỗi lệnh vẫn còn `Resting`, chương trình:
  1. Chuyển giá của lệnh thành tick.
  2. **Gập** (cộng) khối lượng của lệnh vào đúng hộp thư histogram.
  3. Đánh dấu lệnh là `Accumulated` (để không bao giờ bị gập hai lần).
  4. Ghi lại một ảnh chụp `cum_before` — giá trị của rổ *ngay trước khi* lệnh này
     được cộng vào. Ảnh chụp này dùng sau cho việc phân bổ công bằng (xem
     [mục 6](#6-phân-bổ-tại-tick-biên)).
- Nó tăng hai bộ đếm: `accumulated_order_count` trên market và trên histogram.

**Chuyển pha:** lần gọi `process_chunk` đầu tiên chuyển market từ `Collect` sang
`Accumulating` — nhưng chỉ **sau khi cửa sổ thu thập đã đóng** (nó kiểm tra
`Clock.slot >= phase_deadline_slot`). Điều này giữ sổ lệnh mở suốt cửa sổ, để mọi
lệnh trong cửa sổ đều vào cùng một lô.

**Vì sao an toàn:** vì gập có tính giao hoán (phép cộng), hai cranker gập hai chunk
khác nhau theo bất kỳ thứ tự nào đều cho ra cùng một histogram. Cờ `Accumulated`
ngăn việc gập hai lần (đếm một lệnh hai lần). Bước kiểm tra tính đầy đủ ở pha tiếp
theo ngăn việc bỏ sót (bỏ qua một lệnh).

(Tệp: `instructions/process_chunk/processor.rs`.)

### Pha 2 — DISCOVER (`finalize_clear`)

**Mục tiêu:** tìm giá clearing duy nhất và ghi kết quả. Đây là **một giao dịch**.

- Đầu tiên, một **bước kiểm tra tính đầy đủ (completeness check)**. Chương trình từ
  chối chạy trừ khi **mọi lệnh đang hoạt động đã được gập đúng một lần**. Nó kiểm
  tra theo hai cách:
  1. Một gợi ý bộ đếm nhanh: `accumulated_order_count == active_order_count` (và
     tương tự cho maker quotes).
  2. Một lần quét order slab thực sự, đáng tin cậy: `all_active_orders_accumulated`
     xác nhận **không còn ô nào ở trạng thái `Resting`**. Lần kiểm tra thứ hai này
     mới là sự bảo đảm thật — nó không tin các bộ đếm mà nhìn vào dữ liệu thực.
  - Nếu còn gì chưa gập, nó trả về `AuctionNotComplete`.
- Sau đó nó đọc bốn vùng của histogram thành các mảng và chạy phép toán clearing:
  **`find_cross`** (viên ngọc quý, trong `clearing.rs`). Nó chạy **hai lần** — một
  lần cho phiên bid, một lần cho phiên ask ([mục 5](#5-phiên-đấu-giá-kép)).
- Nó ghi một tài khoản `ClearingResult` chứa giá clearing, khối lượng đã khớp, và
  các hằng số phân bổ cho cả hai phiên.
- Nó chuyển pha sang `Discovered` và ghi lại các giá khớp gần nhất.
- **Phí crank (tùy chọn):** nếu cranker cung cấp tài khoản collateral của mình và
  vault, chương trình trả cho họ một `crank_fee` cố định từ quỹ bảo hiểm, như phần
  thưởng cho công sức. Điều này bảo toàn (tiền dịch chuyển bên trong vault, không
  được tạo thêm).

**Một bảo vệ chống DoS quan trọng:** chương trình **không tin** byte bump mà người
gọi gửi cho PDA `ClearingResult`. Nó tự tính lại địa chỉ chuẩn bằng
`find_program_address`. Nếu kẻ gọi cố tạo kết quả ở địa chỉ sai, một `settle_fill`
sau đó sẽ từ chối nó và market sẽ kẹt vĩnh viễn. Bằng cách tự tính địa chỉ, chương
trình ngăn việc từ chối dịch vụ vĩnh viễn này.

(Tệp: `instructions/finalize_clear/processor.rs`.)

### Pha 3 — SETTLE (`settle_fill`)

**Mục tiêu:** trao phần khớp cho từng trader. Đây là **một giao dịch cho mỗi người
dùng**.

Phần khớp được **kéo (pull), không phải đẩy (push)**. Mỗi `settle_fill` thanh toán
đúng một lệnh, nên chi phí ghi vào vị thế của người dùng đó được trả trong **chính
giao dịch của người đó**. Điều này dàn trải công việc qua nhiều giao dịch thay vì
một giao dịch khổng lồ.

Với một lệnh, chương trình:

1. Tìm lệnh trong slab (dùng `order_id` và một `slot_hint`).
2. Kiểm tra lệnh ở trạng thái `Accumulated` (đã gập nhưng chưa thanh toán).
3. Chọn lệnh thuộc phiên nào (lệnh bán → phiên bid, lệnh mua → phiên ask).
4. Tính lượng khớp bằng **`fill_against_cross`** — bộ phân loại khớp duy nhất dùng
   chung (nên khớp của taker và maker luôn dùng đúng cùng một ranh giới và không
   bao giờ lệch nhau).
5. Đánh dấu lệnh `Consumed` và giảm `count` của slab.
6. **Giải phóng** phần margin tệ-nhất đã được giữ lại khi lệnh được gửi.
7. Nếu `fill > 0`, nó **bắt buộc phải có tài khoản position** và áp dụng giao dịch:
   cập nhật size, giá vào trung bình, PnL đã thực hiện, funding, và social loss.
8. Nếu có tài khoản collateral và vault, nó dồn PnL đã thực hiện, tính phí (hoặc
   hoàn phí), khóa lại margin theo size mới, và bảo toàn tiền qua quỹ bảo hiểm.

**Một quy tắc an toàn then chốt:** một phần khớp khác 0 **không bao giờ bị âm thầm
vứt bỏ**. Tài khoản position là **bắt buộc** mỗi khi `fill > 0`. Nếu không, một
cranker độc hại có thể "tiêu thụ" giao dịch đã khớp của bạn với tài khoản rỗng và
hủy nó. Chỉ lệnh **khớp 0** (không khớp gì) mới được tiêu thụ rẻ tiền mà không cần
position.

(Tệp: `instructions/settle_fill/processor.rs`.)

---

## 5. Phiên đấu giá kép

Tempo không chạy một phiên đấu giá — nó chạy **hai phiên cùng lúc**, đó là lý do nó
được gọi là **Dual** (kép) Flow Batch Auction.

Có hai loại người tham gia:

- **Takers** — trader bình thường gửi lệnh (`submit_order`). Họ muốn giao dịch
  ngay.
- **Makers** — nhà cung cấp thanh khoản, đặt báo giá thường trực (`MakerQuote`). Họ
  cung cấp thanh khoản.

Hai phiên là:

1. **Phiên bid** = maker-mua (cầu) vs taker-bán (cung).
2. **Phiên ask** = taker-mua (cầu) vs maker-bán (cung).

Để giữ chúng tách biệt, histogram có **bốn vùng** (`NUM_REGIONS = 4`):

| Vùng         | Được điền bởi          | Ý nghĩa                       |
|--------------|------------------------|-------------------------------|
| `BidDemand`  | báo giá mua của maker  | bên cầu của phiên bid         |
| `BidSupply`  | lệnh bán của taker     | bên cung của phiên bid        |
| `AskDemand`  | lệnh mua của taker     | bên cầu của phiên ask         |
| `AskSupply`  | báo giá bán của maker  | bên cung của phiên ask        |

- Lệnh taker (từ `submit_order`) là **chỉ-taker** và chỉ gập vào `BidSupply` (lệnh
  bán) hoặc `AskDemand` (lệnh mua).
- Maker quotes chỉ gập vào `BidDemand` (lệnh mua của họ) và `AskSupply` (lệnh bán
  của họ).

Trong `finalize_clear`, chương trình chạy `find_cross` một lần cho phiên bid
(`BidDemand` vs `BidSupply`) và một lần cho phiên ask (`AskDemand` vs `AskSupply`).
Nó công bố **cả hai** giá trong `ClearingResult`. Mỗi lệnh thanh toán theo phiên
của riêng nó.

---

## 6. Phân bổ tại tick biên

Đây là một phần toán học tinh tế nhưng rất quan trọng. Nó trả lời: **khi không đủ
khối lượng để khớp cho mọi người tại giá clearing, ai được khớp, và bao nhiêu?**

### Bối cảnh

`find_cross` tìm tick clearing nơi cầu và cung tích lũy giao nhau. Tại "tick biên"
chính xác đó, một bên thường có **nhiều** khối lượng hơn bên kia. Bên nhỏ hơn được
khớp toàn bộ; bên lớn hơn phải bị **phân bổ** (chia sẻ).

Các quy tắc mà `fill_against_cross` áp dụng:

- Các lệnh **tốt hơn hẳn** tick biên (lệnh mua cao hơn nó, lệnh bán thấp hơn nó)
  được **khớp toàn bộ**. Chúng cạnh tranh nên luôn được khớp.
- Bên **khan hiếm** (bên nhỏ hơn) tại tick biên được **khớp toàn bộ**.
- Bên **bị phân bổ** (bên lớn hơn) tại tick biên được khớp **theo tỷ lệ
  (pro-rata)**, dùng `compute_marginal_fill`.

### Mẹo bảo toàn (làm tròn dạng "lồng nhau")

Nguy hiểm của việc chia theo tỷ lệ là **làm tròn**. Nếu bạn làm tròn phần của mỗi
người một cách độc lập, các phần có thể không cộng lại thành tổng — bạn có thể tạo
ra hoặc làm mất khối lượng. Điều đó sẽ phá vỡ sổ sách.

Tempo tránh điều này bằng công thức **làm tròn xuống tích lũy lồng nhau (telescoping
cumulative-floor)**. Mỗi lệnh nhớ `cum_before` — bao nhiêu khối lượng đứng trước nó
trong cùng rổ (được ghi lại lúc ACCUMULATE). Phần khớp của nó là:

```
fill = floor((cum_before + qty) × V / Q) − floor(cum_before × V / Q)
```

trong đó `V` là khối lượng được phân bổ cho tick biên và `Q` là tổng khối lượng tại
đó.

Vì "điểm kết thúc" của mỗi người là "điểm bắt đầu" của người kế tiếp, các phép làm
tròn **triệt tiêu lẫn nhau trên toàn bộ** (chúng "lồng vào nhau"). Tổng của tất cả
phần khớp **chính xác bằng `V`** — không hơn không kém một đơn vị. Không có khối
lượng nào bị tạo ra hay mất đi. Bất kỳ mất mát làm tròn nhỏ nào (nhiều nhất một đơn
vị "bụi") đều làm tròn **bất lợi cho người dùng**, không bao giờ bất lợi cho giao
thức.

Điều này cũng nghĩa là kết quả **độc lập với thứ tự thanh toán**. Ai thanh toán
trước không quan trọng; tổng luôn được bảo toàn. (Các hàm: `find_cross`,
`compute_marginal_fill`, `fill_against_cross` trong `clearing.rs`. Chúng có nhiều
unit test cộng với các test fuzz so sánh với 20.000+ vòng lặp.)

---

## 7. Vòng đời phiên đấu giá

Một market tái sử dụng cùng các tài khoản qua từng vòng. Một market đi qua một **máy
trạng thái (phase machine)**:

```
Collect  →  Accumulating  →  Discovered  →  Settling  →  (vòng kế) Collect
   0            1               2              3
```

- **Collect (0):** sổ mở. Trader gửi và hủy lệnh. Maker cập nhật báo giá. Cửa sổ
  mở trong `COLLECT_WINDOW_SLOTS = 2` slot.
- **Accumulating (1):** cranker gập lệnh vào histogram (`process_chunk`,
  `process_maker_quote`).
- **Discovered (2):** `finalize_clear` đã tìm ra giá và ghi kết quả.
- **Settling (3):** mỗi trader kéo phần khớp của mình (`settle_fill`,
  `settle_maker_quote`).

### Chuyển sang vòng kế (`start_auction`)

`start_auction` là **không cần quyền** và chuyển market sang vòng kế. Nó chỉ thành
công khi vòng trước đã **thanh toán xong hoàn toàn** (pha là `Settling` hoặc
`Discovered`, **và order slab rỗng** — mọi lệnh đều `Consumed`). Rồi nó:

- Tăng `current_auction_id`.
- **Đưa về 0** các rổ histogram và các ô slab (để các ô `Consumed` được tái sử dụng
  — nếu không chúng không bao giờ được giải phóng).
- Reset các bộ đếm và mở lại `Collect`.
- **Căn giữa lại cửa sổ tick theo giá oracle hiện tại** (cố gắng tốt nhất): nó đọc
  một giá oracle mới, đã kiểm tra độ tin cậy, và gọi `recenter_window`. Nếu oracle
  cũ hoặc không chắc chắn, nó giữ cửa sổ cũ và việc chuyển vòng vẫn thành công. Một
  feed tồi **làm chậm** việc căn giữa lại, chứ không bao giờ **dừng** market.

### "Mô hình đóng băng" (không pipeline)

Một vòng mới **không thể mở cho đến khi vòng trước được thanh toán hoàn toàn**.
Không có sự chồng lấn giữa các vòng. Đây là một lựa chọn thiết kế có chủ đích
(system-design §7). Chế độ hỏng là **trễ, không phải mất**: nếu không ai crank,
vòng chỉ đơn giản là chờ. Bất kỳ ai cũng có thể nhảy vào và tiếp tục crank, vì tất
cả lệnh crank đều không cần quyền.

### Reset khẩn cấp (`force_reset`)

Nếu một vòng bị kẹt theo cách tệ, **authority của market** (admin) có thể gọi
`force_reset`. Đây là lối thoát hiểm **duy nhất** không phải không-cần-quyền. Nó
tăng auction id và reset vòng về `Collect` dùng cùng hàm trợ giúp dùng chung
`reset_round_to_collect` mà `start_auction` cũng dùng.

---

## 8. Cách tổ chức code

Chương trình được xây bằng **Pinocchio** — một framework `no_std`, zero-copy, không
phụ thuộc, cho chương trình Solana. "Zero-copy" nghĩa là chương trình đọc dữ liệu
tài khoản trực tiếp tại chỗ, không sao chép sang cấu trúc mới (tiết kiệm compute quý
giá). Nó dùng **Codama** để sinh IDL (bản mô tả giao diện mà client dùng).

### Luồng code

```
lib.rs           khai báo program ID, các module, #![no_std]
   ↓
entrypoint.rs    đọc byte discriminator (1 byte), định tuyến tới handler process_*
   ↓
instructions/*/  một thư mục mỗi lệnh: accounts.rs · data.rs · processor.rs
   ↓
clearing.rs      toán clearing thuần (find_cross, fill_against_cross, ...)
state/*.rs       các struct tài khoản zero-copy
```

### Các quy tắc bố cục nghiêm ngặt (conventions)

Codebase tuân theo phong cách nhất quán, nghiêm ngặt. Khi thêm gì đó, bạn **soi
theo lệnh hiện có gần nhất**:

- **Không có logic trong `mod.rs`** — chỉ khai báo module.
- **Mọi kiểm tra nằm trong `TryFrom`.** `accounts.rs` kiểm tra tài khoản; `data.rs`
  kiểm tra và phân tích dữ liệu lệnh; `processor.rs` chỉ chứa **logic nghiệp vụ**.
  Vì vậy kiểm tra và logic được tách biệt rõ ràng.
- **Không có số thực (floating point) ở bất cứ đâu.** Chỉ dùng `u64` / `u128` (và
  `i128`) với phép toán có-kiểm-tra hoặc bão-hòa. Luôn **làm tròn bất lợi cho người
  dùng**.
- **Không có số ma thuật** — mọi hằng số đều được đặt tên.
- **Cranker không-cần-quyền bị coi là đối thủ.** Tính đúng đắn phải đến từ toán học
  (giao hoán + đầy đủ), không bao giờ từ việc tin người gọi.
- **Một nguồn sự thật duy nhất** — program ID là `crate::ID`, được tham chiếu mọi
  nơi; không bao giờ sao chép.

---

## 9. Mô hình tài khoản

Toàn bộ trạng thái nằm trong **PDA** (Program Derived Address — tài khoản do chương
trình kiểm soát). Mọi struct trạng thái đều là **zero-copy** `#[repr(C)]` với tiền
tố 2 byte: **1 byte discriminator** (cho biết đây là kiểu gì) + **1 byte version**
(phiên bản bố cục). `assert_no_padding!` đảm bảo struct không có byte đệm ẩn.

Dưới đây là tất cả tài khoản:

### `Market` (disc 1, tài khoản chính)

Seeds: `[b"market", market_seed]`. Đây là cấu hình và trạng thái trung tâm cho một
market. Nó giữ (cùng nhiều trường khác):

- Trạng thái đấu giá: `current_auction_id`, `phase`, `phase_deadline_slot`.
- Cấu hình histogram: `tick_size`, `num_ticks`, `window_floor_price`.
- Sổ sách lệnh: `accumulated_order_count`, `active_order_count`,
  `orders_per_auction_cap`.
- Giá gần nhất: `last_bid_fill_price`, `last_ask_fill_price`.
- Cấu hình rủi ro: `maintenance_margin_bps`, `initial_margin_bps`,
  `liquidation_penalty_bps`, `max_position_notional`.
- Phí: `maker_fee_bps`, `taker_fee_bps` (có dấu — âm nghĩa là hoàn phí/rebate),
  `integrator_share_bps`, `crank_fee`.
- Funding: `funding_index` (i128), `last_funding_ts`.
- Oracle: `oracle`, `oracle_feed_id`, `collateral_mint`.
- Gia cố rủi ro: `oi_long`, `oi_short` (open interest, u128),
  `social_loss_index_long`, `social_loss_index_short` (i128), các trường giá hiệu
  dụng có hãm, `max_price_move_bps_per_slot`, `soft_stale_slots`.
- Bộ đếm maker quote: `next_quote_id`, `active_maker_quote_count`,
  `folded_maker_quote_count`.

Các phương thức chính: `price_to_tick` / `tick_to_price` (ánh xạ giá ↔ tick dùng
window floor), `recenter_window` (căn cửa sổ theo oracle mỗi vòng),
`advance_effective_price` (hãm giá), `apply_oi_delta` (giữ open interest đồng bộ),
`socialize_bad_debt` (ADL cho bên thắng), `validate_price`.

### `AuctionHistogram` (disc 2, các "hộp thư")

Seeds: `[b"histogram", market]`. Một header cộng với vùng `4 × num_ticks` các rổ
`u64`. `fold_buy` / `fold_sell` làm phép cộng có-kiểm-tra, giao hoán. Kích thước
của nó chỉ phụ thuộc số tick, **không** phụ thuộc số lệnh.

### `OrderSlab` (disc 4)

Seeds: `[b"orderslab", market]`. Một mảng các ô `Order`, giới hạn bởi
`orders_per_auction_cap`. Mỗi `Order` (88 byte) giữ price, quantity, remaining,
order_id, trader, side, status (`Empty=0` / `Resting=1` / `Accumulated=2` /
`Consumed=3`), ảnh chụp `cum_before`, và `reserved_margin`. Header có con trỏ
`next_free_hint` để cấp phát ô nhanh O(1). Các hàm trợ giúp tìm ô trống, tra lệnh
theo id, kiểm tra tính đầy đủ, và tính tổng tiền tố (prefix sum).

### `ClearingResult` (disc 3)

Seeds: `[b"clearing", market]`. Một kết quả nhỏ, cố định, giữ cho **cả** phiên bid
và ask: giá clearing, khối lượng đã khớp, khối lượng phân bổ cho tick biên, tổng
khối lượng tại tick biên, chỉ số tick biên, và bên nào bị phân bổ. Mỗi người dùng
đọc các hằng số này để tự tính phần khớp của mình.

### `Position` (disc 5, version 3)

Seeds: `[b"position", market, owner]`. Vị thế của một trader trong một market:

- `size` (i64 có dấu: dương = long, âm = short),
- `entry_price` (giá vào trung bình, một VWAP),
- `collateral` (margin bị khóa cho vị thế này),
- `realized_pnl` (i128), `last_funding_index` (i128),
- `last_social_index` (i128, cho ADL — thêm ở version 2),
- `margin_mode` (0 = isolated/độc lập, 1 = cross/chéo — thêm ở version 3).

Phương thức: `apply_fill` (cập nhật VWAP và thực hiện PnL khi giảm/đảo chiều),
`settle_funding`, `settle_social_loss` (chỉ tính cho bên hiện tại — không bao giờ
ghi có), `snapshot_social_index`.

### `UserCollateral` (disc 7)

Seeds: `[b"collateral", owner]`. Sổ tiền của một trader:
`balance`, `locked`, và `free() = balance − locked`. Phương thức: `credit`,
`debit`, `lock`, `release`, `apply_pnl` (trả về phần nợ xấu nếu khoản lỗ lớn hơn số
dư).

### `Vault` (disc 6, version 2)

Seeds: `[b"vault", collateral_mint]`. Quỹ collateral dùng chung:
`collateral_mint`, `vault_token_account`, `insurance_balance`, `authority_bump`,
`bump`. PDA authority của vault (`[b"vault_authority", ...]`) ký các giao dịch
chuyển token ra khỏi vault.

### `MarginAccount` (disc 9)

Seeds: `[b"margin", owner]`. Một nhóm cross-margin: tối đa
`MAX_CROSS_POSITIONS = 8` khóa vị thế thành viên chia sẻ một sổ `UserCollateral`.
(Nó **không** nằm trong IDL vì mảng cố định `[u8; 256]` của nó không ánh xạ được
sang node Codama; client đọc bố cục của nó trực tiếp.)

### `MakerQuote` (disc 8, version 3)

Seeds: `[b"maker_quote", market, maker]`. Báo giá tham số thường trực của một maker
(một thang giá/ladder). Xem [mục 17](#17-maker-quotes).

---

## 10. Toàn bộ tập lệnh

Byte đầu tiên của mỗi lệnh là một **discriminator** chọn handler
(`entrypoint.rs`). Đây là danh sách đầy đủ:

| # | Tên | Làm gì |
|---|------|--------------|
| 0 | `InitializeMarket` | Tạo một market + histogram + order slab của nó |
| 1 | `SubmitOrder` | Một taker gửi lệnh (pha Collect) |
| 2 | `CancelOrder` | Một trader hủy lệnh đang chờ (pha Collect) |
| 3 | `ProcessChunk` | ACCUMULATE: gập một khúc lệnh vào histogram |
| 4 | `FinalizeClear` | DISCOVER: tìm giá clearing, ghi kết quả |
| 5 | `SettleFill` | SETTLE: kéo phần khớp của một lệnh |
| 6 | `StartAuction` | Chuyển sang vòng kế |
| 7 | `InitPosition` | Tạo tài khoản vị thế của trader |
| 8 | `ReadOracle` | Đọc oracle, tính mark, phát sự kiện (chỉ đọc) |
| 9 | `InitVault` | Tạo quỹ collateral toàn cục |
| 10 | `InitCollateral` | Tạo sổ collateral của trader |
| 11 | `Deposit` | Chuyển token vào, ghi có sổ |
| 12 | `Withdraw` | Ghi nợ sổ, chuyển token ra |
| 13 | `UpdateFunding` | Cộng dồn funding từ oracle vs mark |
| 14 | `Liquidate` | Thanh lý một vị thế không lành mạnh |
| 15 | `ForceReset` | Lối thoát hiểm admin cho vòng bị kẹt |
| 16 | `InitMakerQuote` | Tạo báo giá của maker |
| 17 | `UpdateMakerQuoteMid` | Dời tick trung tâm của báo giá (re-quote O(1)) |
| 18 | `UpdateMakerQuoteLevels` | Thay toàn bộ thang giá của báo giá |
| 19 | `ClearMakerQuote` | Vô hiệu hóa một báo giá |
| 20 | `ProcessMakerQuote` | ACCUMULATE một maker quote vào histogram |
| 21 | `SettleMakerQuote` | SETTLE các phần khớp của một maker quote |
| 22 | `InitMarginAccount` | Tạo một nhóm cross-margin |
| 23 | `AddPositionToMargin` | Thêm một vị thế phẳng (flat) vào nhóm |
| 24 | `WithdrawCross` | Rút dựa trên vốn cross-margin tổng hợp |
| 25 | `LiquidateCross` | Thanh lý một tài khoản cross-margin |
| 26 | `MigrateMarket` | Nâng cấp tài khoản Market cũ (v4 → v5) |
| 27 | `MigratePosition` | Nâng cấp tài khoản Position cũ (v1/v2 → v3) |
| — | `RemovePositionFromMargin` | Bỏ một vị thế phẳng khỏi nhóm |
| — | `CloseMakerQuote` | Đóng một báo giá không hoạt động, hoàn rent |
| 228 | `EmitEvent` | Self-CPI nội bộ dùng để phát sự kiện |

### `submit_order` chi tiết hơn

Khi một taker gửi lệnh trong pha `Collect`:

- Nó kiểm tra pha và giá (giá phải nằm trong cửa sổ histogram).
- **Chống spam:** một trader chỉ được giữ tối đa `MAX_ORDERS_PER_TRADER = 8` lệnh
  đang chờ trong một phiên.
- **Giữ trước margin trước giao dịch (rất quan trọng):** Vì phiên đấu giá theo lô
  chỉ khám phá giá **sau khi** khớp, chương trình giữ lại, tại thời điểm gửi, một
  **chặn trên** của margin mà phần khớp có thể cần. Nó khóa
  `worst_qty × worst_price × initial_bps`. Một lệnh mua có thể khớp tối đa tại giá
  giới hạn của nó; một lệnh bán có thể khớp tối đa tại đỉnh cửa sổ. Bằng cách khóa
  trường hợp tệ nhất ngay, `settle_fill` chỉ bao giờ **giải phóng** margin — nó
  không bao giờ thất bại vì thiếu collateral (điều sẽ làm kẹt cả vòng). Một lệnh
  `reduce_only` chỉ giữ phần mở thêm exposure mới, nên việc đóng vị thế không bao
  giờ bị chặn.
- **Giới hạn kích thước vị thế:** nếu `max_position_notional` được đặt, exposure
  **mới** trường-hợp-tệ-nhất của lệnh bị giới hạn. Một lệnh giảm/đóng thuần thêm 0
  exposure mới và không bao giờ bị chặn.
- Nó ghi lệnh vào một ô trống và tăng các bộ đếm.

### `cancel_order`

Không cần quyền để kích hoạt nhưng trader phải ký. Chỉ trong pha `Collect`. Nó xóa
lệnh, giảm các bộ đếm, và **giải phóng margin đã giữ** dùng cùng hàm trợ giúp dùng
chung `release_order_reservation` mà `settle_fill` cũng dùng (nên hai chỗ giải
phóng không bao giờ lệch nhau).

---

## 11. Luồng tiền

### Vault và sổ

- **`Vault`** là quỹ dùng chung duy nhất. Nó sở hữu một tài khoản token SPL giữ tất
  cả collateral, cộng với một `insurance_balance` dùng để hấp thụ lỗ và trả rebate.
- Mỗi trader có một sổ **`UserCollateral`**: `balance`, `locked`, và
  `free = balance − locked`.

### Nạp (`deposit`)

Trader ký. Chương trình chuyển token từ tài khoản token của trader vào tài khoản
token của vault (một CPI `Transfer` SPL), rồi **ghi có** `balance += amount`. Tài
khoản token của vault được kiểm tra so với địa chỉ lưu trong vault, và chủ sổ được
kiểm tra.

### Rút (`withdraw`)

Trader ký. Chương trình **ghi nợ** sổ (`debit` thất bại nếu số tiền lớn hơn `free`,
nên margin bị khóa được bảo vệ), rồi chuyển token ra khỏi vault tới trader. Giao
dịch chuyển được ký bởi **PDA authority của vault** (seeds
`[b"vault_authority", authority_bump]`).

### Bất biến bảo toàn (conservation invariant)

Quy tắc an toàn cốt lõi của luồng tiền: **`token vault ≥ Σ tất cả balance +
insurance`** mọi lúc. Bất cứ khi nào số dư của trader thay đổi (PnL), thay đổi ngược
lại xảy ra trong quỹ bảo hiểm, nên tiền không bao giờ được tạo ra. Một khoản lời
được tài trợ **từ** insurance và **thất bại đóng (fail closed)**
(`InsuranceInsolvent`) nếu insurance quá nhỏ — chương trình **không bao giờ in
tiền**. Một khoản lỗ cộng dồn **vào** insurance. Logic này nằm trong
`settle_money.rs` dùng chung (`conserve_and_socialize`), được dùng bởi
`settle_fill`, `settle_maker_quote`, `liquidate`, và `liquidate_cross` nên cả bốn
đường đi hành xử giống hệt nhau.

---

## 12. Vị thế, funding, và PnL

### Một vị thế

`Position` lưu một `size` có dấu (long hoặc short), một `entry_price` trung bình,
`collateral` bị khóa, và `realized_pnl` cộng dồn. `apply_fill`:

- **Tăng** vị thế: cập nhật giá vào VWAP.
- **Giảm hoặc đảo chiều**: thực hiện PnL trên phần đã đóng.

### Funding

Funding giữ giá perp bám sát giá thật (oracle). Đây là một khoản thanh toán định kỳ
giữa long và short:

- Nếu giá mark **cao hơn** oracle, long trả cho short (và ngược lại).
- Chương trình dùng một **chỉ số funding đơn điệu** (`funding_index`, i128, được
  scale bởi `FUNDING_SCALE = 1e9`). Mỗi vị thế nhớ giá trị chỉ số tại lần thanh
  toán cuối (`last_funding_index`); phần chênh lệch là số nó nợ hoặc nhận.

`update_funding` là **không cần quyền**. Nó:

- Đọc oracle (phải khớp feed của market, mới trong `MAX_AGE_SECS = 120`s, và qua
  kiểm tra độ tin cậy `DEFAULT_MAX_CONF_BPS = 500`).
- Tính giá mark neo theo oracle trong phạm vi `MARK_BAND_BPS = 500` bps.
- Tính tỷ lệ kỳ: `period_fraction_bps = (elapsed × 10000 /
  FUNDING_INTERVAL_SECS).min(10000)`, với `FUNDING_INTERVAL_SECS = 3600` (1 giờ),
  giới hạn ở `MAX_FUNDING_RATE = FUNDING_SCALE / 100` (≈1% mỗi kỳ).
- Nâng `funding_index` và đóng dấu `last_funding_ts`.

Mỗi vị thế thanh toán funding một cách lười (lazy) bên trong `settle_fill` /
`liquidate` qua `settle_funding`, đưa số tiền nợ vào PnL đã thực hiện.

### Toán PnL (không số thực)

- `unrealized_pnl = size × (mark − entry)` (có dấu).
- Phí trên một phần khớp là `signed_protocol_fee` (âm = rebate). Taker trả
  `taker_fee_bps`; maker trả `maker_fee_bps`.
- Các phép nhân lớn dùng `wide_math.rs` (256-bit `mul_div_floor` /
  `mul_div_ceil`) nên `qty × price × bps` không bao giờ tràn dù ở kích thước cực
  lớn.

---

## 13. Giá mark và oracle

### Oracle (`oracle.rs`)

Tempo đọc các feed giá **Pyth** (định dạng `PriceUpdateV2`). Bộ đọc là `no_std` và
phân tích byte trực tiếp. Nó kiểm tra:

- Tài khoản thuộc sở hữu của Pyth receiver (`PYTH_RECEIVER_ID`).
- Feed id khớp feed mà market đã ràng buộc.
- Giá dương và không cũ (cũ hơn `MAX_AGE_SECS = 120`s sẽ bị từ chối) và không từ
  tương lai.
- Độ tin cậy (độ bất định) trong phạm vi `DEFAULT_MAX_CONF_BPS = 500` bps — một giá
  quá bất định sẽ bị từ chối.

Giá được chuẩn hóa về thang cố định `1e8` (`price_1e8`).

### Giá mark (`mark.rs`)

`compute_mark_price` quyết định giá "công bằng" dùng cho rủi ro:

- Nếu cả hai phiên giao nhau → **trung điểm** của hai giá clearing.
- Nếu chỉ một giao nhau → bên đó.
- Nếu không bên nào giao nhau → giá **oracle**.
- Kết quả luôn được **kẹp vào một dải** quanh oracle (±`MARK_BAND_BPS`), nên một
  giá khớp bị thao túng không thể đẩy mark đi quá xa.

### `read_oracle`

Một lệnh chỉ-đọc, không-cần-quyền, đọc oracle trực tiếp, tính mark, và phát một sự
kiện. Nó dùng để chứng minh tích hợp oracle đầu-cuối trên devnet mà không thay đổi
trạng thái nào.

---

## 14. Thanh lý

Một vị thế **có thể bị thanh lý** khi **vốn (equity)** của nó tụt xuống dưới mức
yêu cầu **margin duy trì (maintenance margin)**. `liquidate` là **không cần quyền**
— bất kỳ ai cũng có thể thanh lý một vị thế không lành mạnh và kiếm phần phạt làm
phần thưởng.

Toán học (`margin.rs`, `liquidation_outcome`):

- `maintenance_margin = |size| × mark × maintenance_margin_bps / 10000`.
- `equity = collateral + realized_pnl + unrealized_pnl` (định giá theo mark).
- Có thể thanh lý nếu `equity < maintenance_margin`.
- `penalty = |size| × mark × liquidation_penalty_bps / 10000` → trả cho người thanh
  lý.
- `returned_to_owner = max(0, equity − bad_debt)` → phần còn lại cho chủ.
- `bad_debt` = khoản lỗ vượt quá collateral.

Luồng trong `liquidate`:

1. Đọc oracle (mới → nâng giá hiệu dụng có hãm; cũ-mềm → dùng giá đã đóng băng).
2. Thanh toán funding và social loss của vị thế, rồi **đưa nó về 0** (size,
   collateral, entry, realized).
3. Giải phóng collateral bị khóa của chủ, áp dụng lỗ, trả lại phần thừa nếu có.
4. Ghi có sổ của người thanh lý với phần phạt.
5. Điều chỉnh insurance của vault: insurance hấp thụ nợ xấu tới mức số dư của nó;
   bất kỳ **phần dư** nào vượt insurance được **xã hội hóa (socialize)** cho bên
   thắng theo open interest (ADL — xem mục kế).
6. Cập nhật open interest của market.

---

## 15. Các tính năng gia cố rủi ro

Đây là các tính năng "M3-v1.5" giúp Tempo bền vững dưới áp lực.

### Theo dõi open interest (OI)

Market theo dõi `oi_long` và `oi_short` (tổng size long và short). Mỗi phần khớp và
thanh lý gọi `apply_oi_delta` để giữ các con số này chính xác. OI được dùng để chia
sẻ lỗ công bằng trong ADL.

### ADL / xã hội hóa lỗ

Khi một vụ thanh lý tạo ra **nợ xấu** lớn hơn quỹ bảo hiểm, khoản lỗ không thể chỉ
biến mất (điều đó sẽ phá vỡ tính bảo toàn). Thay vào đó nó được **xã hội hóa cho bên
thắng** theo tỷ lệ open interest của họ. Đây là **Auto-Deleverage (ADL)**.

Cơ chế dùng một **chỉ số social loss** mỗi bên (`social_loss_index_long`,
`social_loss_index_short`, cả hai i128). `socialize_bad_debt` nâng chỉ số của bên
thắng. Mỗi vị thế sau đó trả phần của mình qua `settle_social_loss`, hàm này chỉ bao
giờ **tính phí** cho bên hiện tại, **không bao giờ ghi có** (nên không thể bị lợi
dụng). Một vị thế mới mở chụp lại chỉ số nên nó không bao giờ trả cho các khoản lỗ
xảy ra trước khi nó tồn tại.

### Cổng chặn khả năng thanh toán cứng (hard solvency gate)

Bất kỳ khoản lời nào sắp được trả ra đều được kiểm tra với quỹ bảo hiểm trước. Nếu
quỹ không đủ chi trả, giao dịch **thất bại đóng** (`InsuranceInsolvent`). Chương
trình không bao giờ trả ra số tiền mà nó không có.

### Hãm giá theo từng slot

`max_price_move_bps_per_slot` cộng với logic **giá hiệu dụng** có hãm
(`advance_effective_price`, `clamp_price_step`) giới hạn mức giá rủi ro có thể dịch
chuyển trong một slot. Điều này ngăn một lần cập nhật bị thao túng gây ra một chuỗi
thanh lý dây chuyền.

### Dự phòng oracle cũ-mềm (soft-stale)

`solvency_mark` (oracle.rs) định nghĩa ba trạng thái:

- **Fresh (mới)** — một giá oracle gần đây bình thường; dùng trực tiếp.
- **Frozen (đóng băng)** — giá hơi cũ (trong `soft_stale_slots`); giá tốt cuối được
  "đóng băng" và dùng, nên market tiếp tục chạy.
- **Hard-stale (cũ-cứng)** — quá cũ; các thao tác rủi ro dừng lại thay vì hành động
  trên dữ liệu tồi.

### Toán notional an toàn tràn số

`wide_math.rs` cung cấp `mul_div_floor` / `mul_div_ceil` 256-bit. Nghĩa là
`quantity × price × bps` không bao giờ tràn, ngay cả với vị thế khổng lồ.

### Chứng minh hình thức (formal verification)

`kani_proofs.rs` chạy bộ kiểm tra mô hình Kani trên các phép toán thô (`find_cross`,
`unrealized_pnl`, `wide_mul`) để **chứng minh** không có panic / tràn / mượn-âm
(underflow). Các đặc tính đúng đắn nặng hơn (mà bộ kiểm tra mô hình không thể khám
phá đầy đủ) được phủ bởi các **test fuzz so sánh** (50k vòng lặp, không phụ thuộc
bên ngoài).

---

## 16. Cross-margin

Mặc định mỗi vị thế được ký quỹ **độc lập (isolated)** — collateral riêng của nó chỉ
hậu thuẫn cho chính nó. **Cross-margin** cho phép một trader gom nhiều vị thế lại để
**lời ở một cái bù lỗ ở cái khác**. Tài khoản được đánh giá bởi **một vốn tổng hợp**
so với **một yêu cầu maintenance tổng hợp**.

### Cách hoạt động

- `init_margin_account` tạo một nhóm `MarginAccount` cho chủ.
- `add_position_to_margin` thêm một vị thế **phẳng (flat)** (size 0, collateral 0)
  vào nhóm và đặt `margin_mode = 1` cho nó. Nó kiểm tra không có lệnh đang trên
  đường (in-flight), nên một lệnh đang chờ không thể thanh toán ở chế độ isolated
  sau khi chế độ đã đổi.
- `remove_position_from_margin` bỏ một thành viên phẳng (và nén mảng lại để ô được
  tái sử dụng).

### Quy tắc đầy đủ (ý tưởng an toàn then chốt)

Bất kỳ thao tác nào rút giá trị ra (`withdraw_cross`, `liquidate_cross`) đều phải
nhìn thấy **mọi vị thế thành viên** của nhóm. Nếu người dùng có thể giấu một vị thế
đang lỗ, họ có thể rút số tiền thực ra họ không có. Nên lệnh **bắt buộc phải cung
cấp tất cả thành viên** và **thất bại đóng** nếu thiếu bất kỳ ai.

Để cho chương trình biết thành viên nào còn sống (có vị thế) vs phẳng, lệnh dùng một
**bitmap `live_mask` u8** — một bit cho mỗi thành viên. Một thành viên **sống** cần
một bộ ba `(position, market, oracle)`; một thành viên **phẳng** chỉ cần tài khoản
`position`. Số tài khoản cung cấp phải đúng bằng `live_count × 3 + flat_count`. (Vì
mask là u8, một nhóm bị giới hạn 8 thành viên.)

### Toán vốn tổng hợp (`cross_margin.rs`)

Mỗi thành viên đóng góp một `LegContribution`:

- `equity = realized + recognized_unrealized − pending`
- `maintenance = |size| × mark × bps / 10000`

Núm điều khiển duy nhất `credit_unrealized_gains` phân biệt hai bên gọi:

- **Thanh lý** dùng `true`: nó định giá theo giá thật, nên cả lời và lỗ đều tính vào
  việc tài khoản có đang âm hay không.
- **Rút** dùng `false` (**quy tắc lời-có-hậu-thuẫn / backed-profit rule**): chỉ
  **lỗ** mới trừ vào vốn; lời **trên giấy** chưa thực hiện **không được tính** vào
  số bạn được rút. Bạn không thể rút lời chưa được hậu thuẫn bởi tiền thật đã thanh
  toán.

`liquidate_cross` đóng **một** vị thế thành viên mỗi lần gọi (cái sống đầu tiên),
thực hiện PnL của nó, tính phần phạt, và xã hội hóa bất kỳ phần thiếu hụt nào — gọi
lặp lại sẽ tháo gỡ tài khoản từng bước có giới hạn.

---

## 17. Maker quotes

Maker cung cấp thanh khoản không phải bằng các lệnh đơn mà bằng một **báo giá tham
số hóa** — một **thang giá (ladder)** được mô tả bởi vài tham số. Đây là
`MakerQuote`.

### Thang giá

Một báo giá được neo vào một tick trung tâm `mid_tick`. Mỗi mức (level) `k` có một
`offset` và một `size`:

- Mức bid `k` nằm tại `mid_tick − offset_k` (một lệnh mua dưới trung tâm).
- Mức ask `k` nằm tại `mid_tick + offset_k` (một lệnh bán trên trung tâm).

Có tối đa `MAX_LEVELS = 8` mức mỗi bên. Phần tuyệt vời: để báo giá lại (dời tất cả
giá của bạn), maker chỉ đổi `mid_tick` — đó là **O(1)** (`update_maker_quote_mid`).
Bản thân các mức hiếm khi đổi (`update_maker_quote_levels` thay chúng hoàn toàn khi
cần).

Một báo giá có một `delegate` (người được sửa thang nhưng không bao giờ được chuyển
tiền), một nonce `sequence` (chống phát lại — mỗi lần sửa phải dùng số cao hơn), và
một đồng hồ `expiry_slots` (báo giá bị bỏ qua nếu nó quá cũ).

### Gập maker quotes (`process_maker_quote`)

Đây là bên maker của ACCUMULATE. Với mỗi báo giá đang hoạt động, chưa-gập, chưa-hết
hạn, cranker gập:

- Mỗi mức bid vào `BidDemand[mid_tick − offset]`.
- Mỗi mức ask vào `AskSupply[mid_tick + offset]`.

Nó chụp một **ảnh chụp** `cum_before` cho mỗi mức (giá trị rổ trước lần gập này) để
phân bổ công bằng sau. Các mức rơi khỏi lưới giá bị bỏ qua và giữ sentinel
`SNAPSHOT_UNFOLDED` (= `u64::MAX`), nên chúng khớp **0** trong thanh toán — một mức
chưa-bao-giờ-gập không bao giờ tạo ra vị thế.

**Tính bất biến gập-một-lần (fold-once idempotency):** mỗi báo giá lưu
`folded_auction_id`. Nếu nó đã bằng vòng hiện tại, gập là no-op. Sau khi gập, nó đặt
id và tăng `folded_maker_quote_count` (dùng bởi bước kiểm tra đầy đủ trong
`finalize_clear`).

### Thanh toán maker quotes (`settle_maker_quote`)

Đây là bên maker của SETTLE. Với mỗi mức nó gọi **cùng** bộ phân loại
`fill_against_cross` mà đường taker dùng (nên khớp của maker và taker không bao giờ
lệch và ngừng cân bằng với khối lượng đã khớp). Nó dùng ảnh chụp đã lưu của mỗi mức
cho việc phân bổ tại tick biên. Nó cộng các phần khớp bid và ask, áp dụng chúng vào
vị thế của maker (funding, social loss, VWAP, PnL đã thực hiện), tính phí maker, khóa
lại margin, và bảo toàn tiền qua insurance.

**Tính bất biến thanh-toán-một-lần** dùng `settled_auction_id`. Nó cũng **bắt buộc**
báo giá phải đã được gập trong vòng này (`folded_auction_id == current_auction_id`),
nếu không nó thất bại — một báo giá không thể thanh toán một vòng mà nó chưa từng
tham gia.

### Các hàm trợ giúp vòng đời

- `init_maker_quote` — tạo báo giá, đăng ký nó hoạt động.
- `clear_maker_quote` — vô hiệu hóa (status = 0), đưa thang về 0, giảm số đang hoạt
  động. Tài khoản vẫn còn (rent bị kẹt) cho đến khi được đóng.
- `close_maker_quote` — đóng một báo giá không hoạt động và hoàn rent cho maker.

---

## 18. Nâng cấp tài khoản

Vì các tài khoản on-chain đã tiến hóa qua các phiên bản, chương trình có thể **nâng
cấp tài khoản cũ tại chỗ** mà không mất dữ liệu. Migration làm tài khoản lớn lên
(`realloc`), đưa phần đuôi mới về 0, điền vào bất kỳ trường nào cần giá trị, và nâng
byte version.

- **`migrate_market`** (disc 26): nâng cấp một Market **version 4** lên **version
  5** (khối rủi ro: OI, chỉ số social-loss, giá hiệu dụng, hãm giá, cấu hình
  soft-stale; cộng các trường window-floor và pre-trade-risk thêm sau). Nó cần
  authority. Sau khi nó chạy, `oi_long`/`oi_short` bắt đầu ở 0 và được dựng lại khi
  các vị thế migrate.
- **`migrate_position`** (disc 27): nâng cấp một Position **version 1 hoặc 2** lên
  **version 3**. Nó cần chủ sở hữu. Một nâng cấp v1 cũng **cộng lại size của vị thế
  vào open interest của market** (vì `migrate_market` đã reset OI về 0). Nó yêu cầu
  order slab rỗng (tĩnh lặng) nên không có thanh toán đang trên đường nào có thể đua
  với việc dựng lại OI.

Cả hai migration nhắm tới **đúng phiên bản trước đó** — chúng kiểm tra byte version
trước và từ chối bất kỳ thứ gì khác. Luôn xác minh version của một tài khoản đã
triển khai trước khi migrate.

---

## 19. Các đặc tính an toàn

An toàn của Tempo **không** phụ thuộc vào việc tin những người gửi giao dịch crank.
Nó đến từ toán học và các bước kiểm tra nghiêm ngặt:

1. **Tính giao hoán.** Gập lệnh vào histogram là phép cộng số nguyên, nên histogram
   cuối cùng giống hệt nhau bất kể ai crank, theo thứ tự nào. Một cranker độc hại
   không thể thao túng giá bằng cách xếp thứ tự.

2. **Tính đầy đủ.** `finalize_clear` từ chối chạy cho đến khi **mọi** lệnh được gập
   — và nó xác nhận điều này bằng cách tự quét slab, không chỉ tin một bộ đếm. Tấn
   công crank còn lại duy nhất là **kiểm duyệt (censorship)** (từ chối gập một
   lệnh), và điều đó chỉ gây **trễ**, vì bất kỳ ai khác cũng có thể gập nó.

3. **Bảo toàn chính xác.** Việc làm tròn lồng nhau trong `compute_marginal_fill`
   đảm bảo tổng các phần khớp bằng đúng khối lượng đã khớp. Open interest được bảo
   toàn. Làm tròn luôn **bất lợi cho người dùng**, không bao giờ bất lợi cho giao
   thức.

4. **Tiền thất-bại-đóng.** Vault không bao giờ trả ra nhiều hơn số nó có
   (`InsuranceInsolvent`). Lời được tài trợ từ insurance; lỗ và nợ xấu đi vào
   insurance hoặc được xã hội hóa qua ADL. Tiền không bao giờ được tạo ra.

5. **Không mất giao dịch một cách âm thầm.** Một phần khớp khác 0 luôn yêu cầu tài
   khoản position; một giao dịch đã khớp không bao giờ bị một người thanh toán độc
   hại âm thầm vứt bỏ.

6. **Kháng DoS.** `finalize_clear` tự tính địa chỉ `ClearingResult` chuẩn, nên một
   bump tồi không thể làm kẹt market vĩnh viễn.

7. **Không cần quyền nhưng có giới hạn.** Tất cả crank đều mở cho mọi người, nên
   tính sống (liveness) không phụ thuộc vào một bên vận hành; nhưng chúng có giới
   hạn (theo chunk) nên vừa với giới hạn compute của Solana.

---

## 20. Chi tiết cấp thấp

### Macro `le_field!` và bố cục align-1

Đây là một chi tiết tinh tế nhưng quan trọng. Dữ liệu tài khoản được **ép con trỏ
(pointer-cast) tại byte offset 2** (sau 1 byte discriminator + 1 byte version).
Offset 2 **không** căn theo 8 byte. Nếu một struct có một trường `u64` thuần, đọc nó
tại địa chỉ chưa căn là **hành vi không xác định (undefined behavior)** (nó thực sự
đã panic trên host trước khi điều này được sửa).

Cách sửa: mọi số nguyên nhiều byte trong một struct trạng thái zero-copy được lưu
dưới dạng một **mảng byte little-endian** (`[u8; N]`), giữ cho căn lề của struct
bằng 1. Macro `le_field!` sinh các bộ getter/setter để đọc/ghi các mảng byte này
đúng cách. Nên bạn sẽ thấy các trường như `tick_size_le: [u8; 8]` với một bộ truy
cập `tick_size()`. **Quy tắc: khi thêm một trường số vào một struct trạng thái, dùng
`le_field!`, không bao giờ dùng `u64` thuần.**

### Sự kiện (events)

Mọi lệnh thay đổi trạng thái đều phát một sự kiện để các indexer off-chain theo dõi.
Sự kiện được phát qua một **self-CPI** thông qua lệnh `EmitEvent` (discriminator
228). Cách này thân thiện với indexer và tránh việc log bị cắt cụt. Có một PDA
`event_authority`, và mỗi lệnh mang theo các tài khoản đuôi `event_authority` +
`tempo_program`. Các struct sự kiện nằm trong `events/` (`MarketInitialized`,
`OrderSubmitted`, `OrderCancelled`, `ChunkProcessed`, `ClearingFinalized`,
`FillSettled`, và nhiều hơn).

**Quy tắc quan trọng:** một CPI yêu cầu không còn mượn (borrow) tài khoản nào đang
mở — code luôn đọc các trường vào biến cục bộ và bỏ các "vệ sĩ" `try_borrow`
**trước khi** gọi `emit_event`.

### Lỗi (errors)

`errors.rs` định nghĩa `TempoProgramError` (dùng `thiserror` + Codama errors). Mỗi
lỗi chuyển thành một `ProgramError::Custom`. Các ví dụ bạn sẽ gặp:
`AuctionWrongPhase`, `AuctionNotComplete`, `AuctionIdMismatch`,
`InsufficientCollateral`, `InsuranceInsolvent`, `MissingSettleAccounts`,
`PositionLimitExceeded`, `TraderOrderCapReached`, `InvalidOrderStatus`.

---

## 21. Những phần còn thiếu

Đây là những thứ **có chủ đích** — chúng là các quyết định được ghi nhận, không phải
lỗi:

- **Cửa sổ tick** là một cửa sổ kích thước cố định được căn giữa theo oracle mỗi
  vòng. Sản phẩm thực tế có thể muốn một cửa sổ động hơn (clearing-protocol §6.4).
- **Hậu thuẫn PnL** là kiểu "v1.1 bảo toàn" — PnL chảy qua quỹ bảo hiểm và được bảo
  toàn, nhưng kiểu mark-to-market **bù trừ theo OI** thật giữa long và short (nơi
  PnL của long và short trực tiếp bù nhau liên tục) là một nâng cấp về sau.
- **Phiên đấu giá kép được triển khai và test đầy đủ trong code** (cả hai lần chạy
  `find_cross`, bốn vùng, cả hai đường thanh toán), nhưng các **mô phỏng
  (simulation) clearing** trên cấu trúc maker/taker kép, và việc kiểm chứng trên
  **devnet thật** (mới chỉ có test LiteSVM), vẫn còn đang chờ.
- Các **câu hỏi nghiên cứu thực sự còn mở** — tranh chấp khóa-ghi histogram, đồng hồ
  kỳ vs clearing nhiều slot, số lệnh tối đa mỗi phiên — là **mục đích của benchmark
  M1**. Chúng là các phép đo cần tạo ra, không phải code cần "sửa".

---

## Tóm tắt một đoạn

**Tempo** là một sàn perps trên Solana thay thế cuộc đua tốc độ của khớp lệnh liên
tục bằng một **phiên đấu giá theo lô**: lệnh được thu thập trong một cửa sổ ngắn và
tất cả được khớp tại **một giá đồng nhất**. Phép màu làm cho điều này rẻ on-chain là
**histogram giá** ("các hộp thư"): sổ lệnh được rút gọn thành các tổng tích lũy theo
từng tick, và gập lệnh vào nó là **phép cộng số nguyên giao hoán**, nên kết quả
giống nhau bất kể ai crank. Clearing được chia thành ba pha không-cần-quyền, có giới
hạn — **ACCUMULATE** (gập), **DISCOVER** (tìm giá, với một bước kiểm tra đầy đủ
nghiêm ngặt), và **SETTLE** (mỗi người dùng kéo phần khớp của mình, được bảo toàn
chính xác bởi việc phân bổ làm-tròn-lồng-nhau). Nó chạy như một **phiên đấu giá
kép** (bên maker + bên taker) trên bốn vùng histogram. Trên bộ máy clearing thuần
túy này là một **hệ thống tiền và rủi ro** đầy đủ: một vault collateral và sổ, giữ
trước margin trước giao dịch, funding và giá mark neo theo oracle, thanh lý, một quỹ
bảo hiểm với **ADL/xã hội hóa lỗ**, một cổng chặn khả năng thanh toán cứng, một hãm
giá, dự phòng oracle cũ-mềm, toán 256-bit an toàn tràn số, giới hạn vị thế,
**cross-margin**, và **maker quotes tham số hóa** — tất cả được xây để tính đúng đắn
đến từ **toán học và các bước kiểm tra nghiêm ngặt**, không bao giờ từ việc tin ai
gửi giao dịch.
