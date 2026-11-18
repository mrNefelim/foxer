"""Render the current saved Swing report JSON as a standalone PDF."""
import argparse
import io
import json
from datetime import datetime, timezone
from pathlib import Path
from xml.sax.saxutils import escape

import matplotlib
matplotlib.use('Agg')
import matplotlib.dates as mdates
import matplotlib.pyplot as plt
from matplotlib import font_manager
from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (Image, PageBreak, Paragraph, SimpleDocTemplate, Spacer,
                                Table, TableStyle)

NAVY = '#142D46'
TEAL = '#087F8C'
GREY = '#5B6D7C'
RED = '#C4474B'
WIDTH = A4[0] - 80


def _utc(ms):
    return datetime.fromtimestamp(ms / 1000, timezone.utc).strftime('%Y-%m-%d %H:%M UTC')


def _num(value, digits=2):
    return f'{value:,.{digits}f}'.replace(',', ' ')


def _register_fonts():
    for name, weight in (('Swing', 'normal'), ('SwingBold', 'bold')):
        path = font_manager.findfont(font_manager.FontProperties(family='DejaVu Sans', weight=weight))
        pdfmetrics.registerFont(TTFont(name, path))
    plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 8,
                         'axes.edgecolor': '#CBD5DF', 'axes.labelcolor': GREY,
                         'xtick.color': GREY, 'ytick.color': GREY, 'grid.color': '#9DADB9'})


def _chart(report):
    points = report['portfolio']['mtm_series']
    fig, axes = plt.subplots(2, 1, figsize=(9.1, 5.0), sharex=True,
                             gridspec_kw={'height_ratios': [2, 1]})
    if points:
        dates = [datetime.fromtimestamp(p['millis'] / 1000, timezone.utc) for p in points]
        equity = [p['gross_equity_r'] for p in points]
        peaks, drawdown = [], []
        peak = 0.0
        for value in equity:
            peak = max(peak, value)
            peaks.append(peak)
            drawdown.append(peak - value)
        axes[0].plot(dates, equity, color=TEAL, linewidth=1.6)
        axes[1].fill_between(dates, [0.0] * len(drawdown), drawdown, color=RED, alpha=.28)
        axes[1].plot(dates, drawdown, color=RED, linewidth=.7)
    axes[0].set_title('Портфельная equity на общих наблюдаемых отметках, R', loc='left', color=NAVY)
    axes[1].set_title('Просадка на отображаемых общих отметках, R', loc='left', color=NAVY)
    axes[1].xaxis.set_major_formatter(mdates.DateFormatter('%Y-%m'))
    for ax in axes:
        ax.axhline(0, color=GREY, linewidth=.6)
        ax.grid(alpha=.18)
        ax.spines[['top', 'right']].set_visible(False)
        ax.tick_params(labelsize=7)
    fig.autofmt_xdate()
    fig.tight_layout(pad=1.2)
    stream = io.BytesIO()
    fig.savefig(stream, format='png', dpi=160, bbox_inches='tight', facecolor='white')
    plt.close(fig)
    stream.seek(0)
    return Image(stream, width=WIDTH, height=330, kind='proportional')


def render(report_path, output_path):
    report_path, output_path = Path(report_path), Path(output_path)
    if output_path.exists():
        raise FileExistsError(f'output already exists: {output_path}')
    report = json.loads(report_path.read_text(encoding='utf-8'))
    if report.get('schema_version') != 1 or tuple(report.get('symbols', ())) != ('EURUSD', 'GBPUSD', 'USDJPY'):
        raise ValueError('unsupported Swing report schema or pair set')
    _register_fonts()
    styles = getSampleStyleSheet()
    styles.add(ParagraphStyle('RuBody', fontName='Swing', fontSize=8.5, leading=12,
                              textColor=colors.HexColor(NAVY), spaceAfter=7))
    styles.add(ParagraphStyle('RuSmall', parent=styles['RuBody'], fontSize=7.2,
                              leading=10, textColor=colors.HexColor(GREY)))
    styles.add(ParagraphStyle('RuTitle', parent=styles['RuBody'], fontName='SwingBold',
                              fontSize=23, leading=28, spaceAfter=10))
    styles.add(ParagraphStyle('RuHeading', parent=styles['RuBody'], fontName='SwingBold',
                              fontSize=14, leading=18, spaceBefore=5, spaceAfter=8))
    styles.add(ParagraphStyle('RuCell', parent=styles['RuBody'], fontSize=7,
                              leading=9, spaceAfter=0))
    styles.add(ParagraphStyle('RuCellHead', parent=styles['RuCell'], fontName='SwingBold',
                              textColor=colors.white))
    story = []

    def para(text, style='RuBody'):
        story.append(Paragraph(escape(str(text)), styles[style]))

    def heading(text):
        para(text, 'RuHeading')

    def table(rows, widths=None):
        if widths:
            widths = [WIDTH * value / sum(widths) for value in widths]
        data = []
        for row_i, row in enumerate(rows):
            style = 'RuCellHead' if row_i == 0 else 'RuCell'
            data.append([Paragraph(escape(str(cell)), styles[style]) for cell in row])
        item = Table(data, colWidths=widths, repeatRows=1, hAlign='LEFT')
        item.setStyle(TableStyle([
            ('BACKGROUND', (0, 0), (-1, 0), colors.HexColor(NAVY)),
            ('ROWBACKGROUNDS', (0, 1), (-1, -1), [colors.white, colors.HexColor('#F4F7F9')]),
            ('VALIGN', (0, 0), (-1, -1), 'TOP'),
            ('LEFTPADDING', (0, 0), (-1, -1), 6), ('RIGHTPADDING', (0, 0), (-1, -1), 6),
            ('TOPPADDING', (0, 0), (-1, -1), 5), ('BOTTOMPADDING', (0, 0), (-1, -1), 5),
            ('LINEBELOW', (0, 0), (-1, 0), .7, colors.HexColor(TEAL)),
        ]))
        story.extend([item, Spacer(1, 8)])

    def notice(text):
        item = Table([[Paragraph(escape(text), styles['RuSmall'])]], colWidths=[WIDTH])
        item.setStyle(TableStyle([('BACKGROUND', (0, 0), (-1, -1), colors.HexColor('#FFF3E8')),
                                  ('LINEBEFORE', (0, 0), (0, 0), 3, colors.HexColor('#D5894B')),
                                  ('LEFTPADDING', (0, 0), (-1, -1), 10),
                                  ('TOPPADDING', (0, 0), (-1, -1), 8),
                                  ('BOTTOMPADDING', (0, 0), (-1, -1), 8)]))
        story.extend([item, Spacer(1, 9)])

    def footer(canvas, doc):
        canvas.saveState()
        canvas.setStrokeColor(colors.HexColor('#D5DEE5'))
        canvas.line(40, 31, A4[0] - 40, 31)
        canvas.setFont('Swing', 7)
        canvas.setFillColor(colors.HexColor(GREY))
        canvas.drawString(40, 20, 'Swing · исторические MT5 M1 данные · расчёт в R')
        canvas.drawRightString(A4[0] - 40, 20, f'{doc.page}')
        canvas.restoreState()

    period = report['period']
    portfolio = report['portfolio']
    terminal = portfolio['terminal']
    para('ОТЧЁТ ПО ТЕКУЩИМ ПРАВИЛАМ SWING', 'RuSmall')
    para(report['title'], 'RuTitle')
    para(f"Период UTC: {_utc(period['from_utc_ms'])} — {_utc(period['to_utc_exclusive_ms'])} (правая граница не включается).")
    para(f"Пары: {', '.join(report['symbols'])}. Настроенный source-grid start: {period['configured_start_source_grid']}; контекст строился с {period['warmup_from_utc']}.")
    notice(report['real_market_data_note'])
    table([
        ['Показатель портфеля', 'Результат'],
        ['Закрытые сделки', str(portfolio['closed'])],
        ['Закрытый результат', f"{_num(portfolio['closed_r'])} R"],
        ['Закрытый drawdown', f"{_num(portfolio['closed_drawdown_r'])} R"],
        ['Terminal marked equity', f"{_num(terminal['marked_equity_r'])} R"],
        ['Terminal realized / open', f"{_num(terminal['realized_r'])} / {_num(terminal['unrealized_r'])} R"],
        ['MTM drawdown на минутных отметках', f"{_num(portfolio['mtm_drawdown_r'])} R"],
        ['Пик одновременных Active-сделок', str(portfolio['peak_open_positions'])],
        ['Active на последней общей отметке', str(terminal['open_positions'])],
    ], [64, 36])
    para(f"Портфельная кривая использует {report['coverage']['common_observations']:,} общих фактически наблюдавшихся M1 закрытий; последний общий mark — {_utc(report['coverage']['common_last_mark_utc_ms'])}. Пропущенные отметки не заполняются.".replace(',', ' '), 'RuSmall')
    story.append(PageBreak())

    heading('Портфель и результаты по парам')
    story.append(_chart(report))
    rows = [['Пара', 'Закрыто', 'Закрытый R', 'Terminal R', 'MTM DD', 'Пик Active']]
    for symbol in report['symbols']:
        item = report['by_symbol'][symbol]
        rows.append([symbol, str(item['closed']), _num(item['closed_r']),
                     _num(item['terminal']['marked_equity_r']), _num(item['mtm_drawdown_r']),
                     str(item['peak_open_positions'])])
    table(rows, [17, 12, 18, 18, 16, 19])
    para('График показывает ежедневные последние общие отметки и сохранённые экстремумы. Число MTM drawdown выше вычислено отдельно по полному общему минутному ряду.', 'RuSmall')
    heading('Закрытые сделки по году выхода')
    rows = [['UTC год', 'Всего', 'R всего', 'EURUSD R', 'GBPUSD R', 'USDJPY R']]
    for year, item in report['yearly_closed'].items():
        rows.append([year, str(item['closed']), _num(item['closed_r'])] +
                    [_num(item['by_symbol'].get(s, {}).get('closed_r', 0.0)) for s in report['symbols']])
    table(rows, [10, 12, 16, 20, 20, 22])
    story.append(PageBreak())

    heading('Параметры рабочего JSON')
    rows = [['Пара', 'RR', 'Цель', 'Лимит target', 'Срок, дни', 'Lookback H4', 'Buffer ATR', 'Cost', 'Range']]
    for symbol in report['symbols']:
        cfg = report['configs'][symbol]
        rows.append([symbol, str(cfg['rr_fixed']), str(cfg['take_profit']), str(cfg['rr_target_cap']),
                     str(cfg['plan_days']), str(cfg['stop_lookback_h4_bars']),
                     str(cfg['stop_buffer_atr']), str(cfg['cost']),
                     'вкл.' if cfg['range']['enabled'] else 'выкл.'])
    table(rows, [12, 8, 11, 12, 10, 14, 11, 10, 12])
    para('Полные per-pair конфигурации сохранены рядом с этим отчётом; хеш каждого указан в provenance JSON.', 'RuSmall')
    heading('Модель расчёта')
    para('Бары W/D/H4 строятся общим агрегатором из подготовленных минутных котировок. Исполнение проверяется по OHLC M1: если одна минута касается и stop, и target, первым считается stop. Это консервативное правило для внутриминутной неоднозначности; фактический путь цены внутри минуты не известен.')
    para('Стоимость из каждой конфигурации один раз учитывает Swing на входе против сделки. Начальный риск сделки равен 1R; R — относительная единица риска, не процент капитала, денежная доходность или оценка маржи.')
    para('MTM строится только по общим наблюдавшимся минутным отметкам всех пар. Для открытых позиций terminal realized и unrealized разделены на последней общей отметке, а не на более поздних индивидуальных котировках.')
    story.append(PageBreak())

    heading('Источники, пропуски и воспроизводимость')
    rows = [['Пара', 'Источник', 'Данные до', 'M1 строк', 'Разрывы', 'Нет месяцев']]
    for symbol in report['symbols']:
        src = report['input_provenance'][symbol]
        rows.append([symbol, 'MT5 user export, bid OHLC', src['last_minute_utc'],
                     str(src['input_file']['rows']), str(src['source_gap_intervals']),
                     str(len(src['missing_months']))])
    table(rows, [10, 27, 22, 14, 12, 15])
    para(f"Цены: {report['price_basis']}; timezone хранения: {report['timezone']}. Источник: {report['source']}.")
    for limitation in report['limitations']:
        para('• ' + limitation, 'RuSmall')
    notice('Ограничение: отчёт использует реальные исторические котировки, но не реальные исполненные брокерские сделки. Он не подтверждает будущую прибыльность или качество live-исполнения.')
    rows = [['Пара', 'Quality SHA-256', 'Replay CSV SHA-256', 'Config SHA-256']]
    for symbol in report['symbols']:
        src = report['input_provenance'][symbol]
        rows.append([symbol, src['quality_sha256'][:16] + '…', src['trade_csv']['sha256'][:16] + '…',
                     src['config_sha256'][:16] + '…'])
    table(rows, [10, 30, 30, 30])
    para(f"План SHA-256: {report['completion']['run_plan_sha256']}", 'RuSmall')
    para(f"Release бинарник SHA-256: {report['completion']['binary_sha256']}", 'RuSmall')
    para(f"SQLite SHA-256: {report['completion']['database_sha256']}", 'RuSmall')
    para(f"Docker image: {report['completion']['docker_image']}", 'RuSmall')
    para('Replay команды и проверенные выходы: run-plan.json и completion.json в каталоге отчёта.', 'RuSmall')

    output_path.parent.mkdir(parents=True, exist_ok=True)
    doc = SimpleDocTemplate(str(output_path), pagesize=A4, rightMargin=40, leftMargin=40,
                            topMargin=40, bottomMargin=42, title=report['title'],
                            author='Forex backtest tooling')
    doc.build(story, onFirstPage=footer, onLaterPages=footer)
    return output_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report_json', type=Path)
    parser.add_argument('output_pdf', type=Path)
    args = parser.parse_args()
    render(args.report_json, args.output_pdf)


if __name__ == '__main__':
    main()
