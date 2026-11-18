"""Presentation only: all strategy decisions and statistics are produced by Rust."""
import io
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from xml.sax.saxutils import escape

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import matplotlib.dates as mdates
from matplotlib.patches import Rectangle
from matplotlib import font_manager
from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.enums import TA_RIGHT
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas
from reportlab.platypus import SimpleDocTemplate, Paragraph, Spacer, Table, TableStyle, Image, PageBreak, Flowable

NAVY = '#142D46'
TEAL = '#087F8C'
RED = '#C4474B'
GREY = '#5B6D7C'
LIGHT = '#EDF3F7'
WIDTH = A4[0] - 96


class MetricCards(Flowable):
    def __init__(self, items):
        Flowable.__init__(self)
        self.items = items
        self.width = WIDTH
        self.height = 77

    def draw(self):
        c = self.canv
        gap = 9
        w = (self.width - gap * (len(self.items)-1)) / len(self.items)
        for i, (label, value, colour) in enumerate(self.items):
            x = i * (w + gap)
            c.setFillColor(colors.HexColor(LIGHT))
            c.roundRect(x, 7, w, 64, 7, fill=1, stroke=0)
            c.setFillColor(colors.HexColor(colour))
            c.roundRect(x+12, 48, 19, 3, 1.5, fill=1, stroke=0)
            c.setFont('Report', 7.5)
            c.setFillColor(colors.HexColor(GREY))
            c.drawString(x+12, 55, label)
            c.setFont('ReportBold', 23)
            c.setFillColor(colors.HexColor(colour))
            c.drawString(x+12, 20, value)



def timestamp(value, millis=True):
    if value is None:
        return '-'
    return datetime.fromtimestamp(value / (1000 if millis else 1), timezone.utc).strftime('%d.%m.%Y %H:%M')


def number(value, digits=2, suffix=''):
    return '-' if value is None else f'{value:,.{digits}f}'.replace(',', ' ') + suffix


def execution_and_sources(data):
    execution = data.get('execution')
    if not isinstance(execution, dict) or not isinstance(execution.get('intrabar'), str):
        method = 'Метод исполнения не указан в сохранённом результате.'
        modes = {}
    else:
        intrabar = execution['intrabar']
        symbols = data.get('symbols', [])
        has_symbol_modes = 'intrabar_by_symbol' in execution
        symbol_modes = execution.get('intrabar_by_symbol')
        if not has_symbol_modes:
            if intrabar == 'observed ticks':
                symbol_modes = {symbol: 'observed ticks' for symbol in symbols}
            elif intrabar == 'synthetic: 4 index quotes per M1, worst-case extreme order for open signals':
                symbol_modes = {symbol: 'synthetic' for symbol in symbols}
            else:
                symbol_modes = {}
        valid_modes = (
            isinstance(symbol_modes, dict)
            and set(symbol_modes) == set(symbols)
            and all(mode in ('observed ticks', 'synthetic') for mode in symbol_modes.values())
        )
        if valid_modes:
            modes = {
                symbol: (
                    'только наблюдаемые тики; внутриминутный путь не восстанавливается'
                    if mode == 'observed ticks' else
                    'четыре синтетические индексные котировки на минуту; экстремумы идут в неблагоприятном порядке для открытых сигналов'
                )
                for symbol, mode in symbol_modes.items()
            }
            distinct = set(symbol_modes.values())
            expected = ('mixed' if len(distinct) > 1 else
                        'synthetic: 4 index quotes per M1, worst-case extreme order for open signals'
                        if distinct == {'synthetic'} else 'observed ticks')
            if intrabar != expected:
                method = 'Режим исполнения противоречит сохранённой карте пар.'
            elif intrabar == 'mixed':
                method = 'В запуске использованы разные модели исполнения; режим указан для каждой пары ниже.'
            elif intrabar == 'synthetic: 4 index quotes per M1, worst-case extreme order for open signals':
                method = 'Модель пути цены: четыре синтетические индексные котировки на минуту; при открытых сигналах экстремумы идут в неблагоприятном порядке.'
            else:
                method = 'Модель пути цены: только наблюдаемые тики; внутриминутный путь не восстанавливается.'
        else:
            modes = {}
            if intrabar == 'mixed' or 'intrabar_by_symbol' in execution:
                method = 'Режим исполнения по парам отсутствует, неполон или противоречив; точное описание невозможно.'
            elif intrabar == 'synthetic: 4 index quotes per M1, worst-case extreme order for open signals':
                method = 'Модель пути цены: четыре синтетические индексные котировки на минуту; при открытых сигналах экстремумы идут в неблагоприятном порядке.'
            elif intrabar == 'observed ticks':
                method = 'Модель пути цены: только наблюдаемые тики; внутриминутный путь не восстанавливается.'
            else:
                method = f'Режим исполнения сохранён, но не распознан: {intrabar}.'
    labels = set()
    sources = data.get('sources')
    if isinstance(sources, dict):
        for entry in sources.values():
            files = entry.get('files') if isinstance(entry, dict) else None
            if not isinstance(files, list):
                continue
            for record in files:
                label = record.get('source') if isinstance(record, dict) else None
                if isinstance(label, str) and label.strip():
                    labels.add(label.strip())
    source_text = ('Источники: ' + '; '.join(sorted(labels)) + '.') if labels else 'Источник котировок не указан в сохранённом результате.'
    return execution if isinstance(execution, dict) else {}, method, source_text, modes


def execution_value(key, value):
    known = {
        ('market', 'first observed index price'): 'первая допустимая наблюдаемая цена индекса',
        ('limit', 'level without improvement'): 'уровень без улучшения цены',
        ('stop', 'observed index price'): 'наблюдаемая цена индекса на стороне выхода',
        ('target', 'level without improvement'): 'уровень без улучшения цены',
        ('costs', 'not modeled'): 'не моделируются',
        ('gaps', 'observed ticks only; affected trades flagged'): 'используются доступные котировки; сделки, затронутые пропусками, отмечаются',
        ('drawdown', 'closed trades only'): 'по закрытым сделкам',
    }
    if key == 'intrabar':
        if value == 'mixed':
            return 'смешанный режим; см. режимы по каждой паре'
        if value == 'observed ticks':
            return 'только наблюдаемые тики'
        if value == 'synthetic: 4 index quotes per M1, worst-case extreme order for open signals':
            return 'четыре синтетические индексные котировки на минуту; неблагоприятный порядок экстремумов для открытых сигналов'
    return known.get((key, value), f'неизвестное значение ({value})')


def figure_image(fig, width=WIDTH, height=270):
    stream = io.BytesIO()
    fig.savefig(stream, format='png', dpi=180, facecolor='white', bbox_inches='tight')
    plt.close(fig)
    stream.seek(0)
    return Image(stream, width=width, height=height, kind='proportional')


def equity_chart(summary, start, end):
    fig, axes = plt.subplots(2, 1, figsize=(9.2, 5.2), sharex=True, gridspec_kw={'height_ratios': [2, 1]})
    curve = summary['curve']
    if curve:
        dates = [datetime.fromtimestamp(start/1000, timezone.utc)] + [datetime.fromtimestamp(x['time']/1000, timezone.utc) for x in curve] + [datetime.fromtimestamp(end/1000, timezone.utc)]
        axes[0].step(dates, [0] + [x['net_r'] for x in curve] + [curve[-1]['net_r']], where='post', color=TEAL if summary['net_r'] >= 0 else RED, lw=2.3)
        axes[1].fill_between(dates, [0] + [-x['drawdown_r'] for x in curve] + [-curve[-1]['drawdown_r']], step='post', color=RED, alpha=.25)
        axes[1].xaxis.set_major_formatter(mdates.DateFormatter('%d.%m.%y'))
    else:
        axes[0].text(.5, .5, 'Сделки не закрывались', transform=axes[0].transAxes, ha='center', color=GREY)
    for ax, title in zip(axes, ['Накопленный результат, R', 'Просадка по закрытым сделкам, R']):
        ax.set_title(title, loc='left', fontsize=11, color=NAVY)
        ax.axhline(0, color=GREY, lw=.6)
        ax.grid(alpha=.15)
        ax.spines[['top', 'right']].set_visible(False)
        ax.tick_params(labelsize=8)
    fig.tight_layout(pad=1.3)
    return figure_image(fig, height=240)


def candles_chart(candles, trade, frame, zone=None):
    fig, ax = plt.subplots(figsize=(9.2, 4.5))
    if candles:
        dates = [mdates.date2num(datetime.fromtimestamp(c['end'], timezone.utc)) for c in candles]
        width = {'H4': .10, 'D': .6, 'W': 3.5}[frame]
        for x, c in zip(dates, candles):
            o, h, low, close = [c[k]/1_000_000 for k in ('open', 'high', 'low', 'close')]
            colour = TEAL if close >= o else RED
            ax.vlines(x, low, h, color=colour, linewidth=.6)
            ax.add_patch(Rectangle((x-width/2, min(o, close)), width, max(abs(close-o), .000001), facecolor=colour, linewidth=0))
        ax.set_xlim(min(dates)-width, max(dates)+width)
        ax.xaxis_date()
        ax.xaxis.set_major_formatter(mdates.DateFormatter('%m.%y' if frame == 'W' else '%d.%m'))
    if zone and 'low' in zone:
        ax.axhspan(zone['low']/1e6, zone['high']/1e6, color=TEAL, alpha=.13, label='Зона решения')
    for key, label, colour in [('entry','Вход',NAVY),('stop','Стоп',RED),('take_profit','Тейк',TEAL)]:
        ax.axhline(trade[key]/1e6, color=colour, linestyle='--', lw=.85, label=f'{label} {trade[key]/1e6:.6f}')
    for key, label in [('entry_at','Вход'),('exit_at','Выход')]:
        if trade.get(key) is not None:
            x = mdates.date2num(datetime.fromtimestamp(trade[key]/1000, timezone.utc))
            if not ax.get_xlim()[0] <= x <= ax.get_xlim()[1]:
                continue
            ax.axvline(x, color=GREY, lw=.6, alpha=.6)
            ax.text(x, .98 if key == 'entry_at' else .04, label, transform=ax.get_xaxis_transform(), fontsize=8, va='top' if key == 'entry_at' else 'bottom', rotation=90)
    ax.set_title(f'{frame} / индекс / время UTC', loc='left', fontsize=10, color=NAVY)
    ax.legend(fontsize=8, loc='best', framealpha=.9)
    ax.ticklabel_format(axis='y', style='plain', useOffset=False)
    ax.tick_params(labelsize=8)
    ax.grid(alpha=.13)
    ax.spines[['top','right']].set_visible(False)
    fig.tight_layout()
    return figure_image(fig, height=265)


def render(directory):
    directory = Path(directory)
    data = json.loads((directory/'results.json').read_text())
    if data['schema_version'] != 1:
        raise ValueError('Unsupported results schema')
    for name, weight in [('Report', 'normal'), ('ReportBold', 'bold')]:
        pdfmetrics.registerFont(TTFont(name, font_manager.findfont(font_manager.FontProperties(family='DejaVu Sans', weight=weight))))
    plt.rcParams.update({'font.family':'DejaVu Sans', 'font.size':9, 'axes.edgecolor':'#CBD5DF', 'axes.labelcolor':GREY, 'xtick.color':GREY, 'ytick.color':GREY, 'grid.color':'#9DADB9'})
    styles = getSampleStyleSheet()
    styles.add(ParagraphStyle('BodyRu', fontName='Report', fontSize=8.5, leading=12, textColor=NAVY, spaceAfter=8))
    styles.add(ParagraphStyle('SmallRu', parent=styles['BodyRu'], fontSize=7, leading=10, textColor=GREY))
    styles.add(ParagraphStyle('TitleRu', parent=styles['BodyRu'], fontName='ReportBold', fontSize=29, leading=35, spaceAfter=12))
    styles.add(ParagraphStyle('HeadingRu', parent=styles['BodyRu'], fontName='ReportBold', fontSize=15, leading=20, spaceBefore=7, spaceAfter=10))
    styles.add(ParagraphStyle('CellRu', parent=styles['BodyRu'], fontSize=7.5, leading=10, spaceAfter=0))
    story = []
    def paragraph(text, style='BodyRu'):
        return Paragraph(escape(str(text)), styles[style])
    def heading(text):
        story.append(paragraph(text, 'HeadingRu'))
    def notice(text):
        box = Table([[paragraph(text, 'SmallRu')]], colWidths=[WIDTH])
        box.setStyle(TableStyle([('BACKGROUND',(0,0),(-1,-1),colors.HexColor('#FFF1E8')), ('LINEBEFORE',(0,0),(0,0),3,colors.HexColor('#D5894B')), ('LEFTPADDING',(0,0),(-1,-1),12), ('TOPPADDING',(0,0),(-1,-1),9), ('BOTTOMPADDING',(0,0),(-1,-1),9)]))
        story.extend([box,Spacer(1,9)])
    def table(rows, widths=None):
        if len(rows) == 1:
            return
        if widths:
            widths = [w * WIDTH / sum(widths) for w in widths]
        header_style = ParagraphStyle('TableHeader', parent=styles['CellRu'], textColor=colors.white, fontName='ReportBold')
        cells = [[Paragraph(escape(str(c)), header_style if i == 0 else styles['CellRu']) for c in row] for i, row in enumerate(rows)]
        tab = Table(cells, colWidths=widths, repeatRows=1, hAlign='LEFT')
        tab.setStyle(TableStyle([('BACKGROUND',(0,0),(-1,0),colors.HexColor(NAVY)),('VALIGN',(0,0),(-1,-1),'TOP'),('LEFTPADDING',(0,0),(-1,-1),8),('RIGHTPADDING',(0,0),(-1,-1),8),('TOPPADDING',(0,0),(-1,-1),6),('BOTTOMPADDING',(0,0),(-1,-1),6),('LINEBELOW',(0,0),(-1,0),.7,colors.HexColor(TEAL)),('ROWBACKGROUNDS',(0,1),(-1,-1),[colors.white,colors.HexColor('#F7F9FB')])]))
        story.extend([tab, Spacer(1,8)])
    s = data['summary']
    execution, execution_label, source_label, execution_modes = execution_and_sources(data)
    story.append(paragraph('ОТЧЁТ ОБ ИСТОРИЧЕСКОМ ТЕСТИРОВАНИИ', 'SmallRu'))
    story.append(paragraph('Стратегия в цифрах', 'TitleRu'))
    story.append(paragraph(f"{timestamp(data['from'])} - {timestamp(data['to'])} UTC · конец периода не включён"))
    story.append(paragraph(' / '.join(data['symbols'])))
    story.append(MetricCards([('Результат закрытых',number(s['net_r'],2,' R'),RED if s['net_r'] < 0 else TEAL),('Просадка закрытых',number(s['max_closed_drawdown_r'],2,' R'),NAVY),('Закрытые сделки',str(s['closed']),NAVY)]))
    story.append(MetricCards([('Доля прибыльных',number(None if s['win_rate'] is None else s['win_rate']*100,1,'%'),NAVY),('Средняя сделка',number(s['mean_r'],2,' R'),NAVY),('Profit factor',number(s['profit_factor']),NAVY)]))
    story.append(equity_chart(s, data['from'], data['to']))
    if not s['closed']:
        story.append(paragraph('Сделки не закрывались. Нулевая выборка не характеризует качество стратегии.'))
    costs = execution_value('costs', execution['costs']) if 'costs' in execution else 'сведения не сохранены'
    drawdown = execution_value('drawdown', execution['drawdown']) if 'drawdown' in execution else 'сведения не сохранены'
    story.append(paragraph(f"{execution_label} Издержки: {costs}. Просадка: {drawdown}.", 'SmallRu'))
    heading('Результаты по парам')
    rows = [['Пара','Закрыто','Итог, R','Просадка, R','Среднее, R']]
    for symbol, row in data['by_symbol'].items():
        rows.append([symbol,str(row['closed']),number(row['net_r']),number(row['max_closed_drawdown_r']),number(row['mean_r'])])
    table(rows,[95]*5)
    story.append(PageBreak())
    heading('Детали и качество данных')
    rows = [['Месяц выхода','Закрыто','Итог, R','Доля прибыльных']]
    for month, row in data['monthly'].items():
        rows.append([month,str(row['closed']),number(row['net_r']),number(None if row['win_rate'] is None else row['win_rate']*100,1,'%')])
    table(rows,[125,90,120,140])
    story.append(paragraph(f"Средняя длительность закрытой сделки: {number(None if s['mean_duration_seconds'] is None else s['mean_duration_seconds']/3600,1)} ч."))

    story.append(paragraph(f"Открыто: {s['open']} · Ожидают входа: {s['pending']} · Отменено: {s['cancelled']}. Позиции в конце периода не закрываются искусственно."))
    rows = [['Пара / статус','Вход','Текущий R','Время цены UTC']]
    for trade in data['trades']:
        if trade['state'] in ('Active','Pending'):
            rows.append([trade['symbol']+' / '+('открыт' if trade['state']=='Active' else 'ожидание'),number(trade['entry']/1e6,6),number(trade.get('unrealized_r')),timestamp(trade.get('marked_at'))])
    table(rows,[120,100,85,170])
    heading('Покрытие котировок')
    story.append(paragraph(source_label, 'SmallRu'))
    if execution_modes:
        mode_lines = [f"{symbol} — {mode}" for symbol, mode in sorted(execution_modes.items())]
        story.append(paragraph('Режим исполнения по парам: ' + '; '.join(mode_lines) + '.', 'SmallRu'))
    if execution.get('intrabar'):
        gap_rule = execution_value('gaps', execution['gaps']) if 'gaps' in execution else 'сведения не сохранены'
        story.append(paragraph(f"Путь цены: {execution_value('intrabar', execution['intrabar'])}. Правило гэпов: {gap_rule}.", 'SmallRu'))
    else:
        story.append(paragraph('Методика обработки внутриминутных цен и гэпов не указана в сохранённом результате.', 'SmallRu'))
    rows = [['Пара','Тиков','Первая / последняя котировка UTC']]
    for symbol, row in data['coverage'].items():
        rows.append([symbol,str(row['ticks']),timestamp(row['first'])+' / '+timestamp(row['last'])])
    table(rows,[85,100,290])
    g = data['gap_affected_summary']
    notice(f"Сигналов с известными пропусками: {s['gap_affected']}. Среди них закрыто {g['closed']}; наблюдаемый итог {number(g['net_r'])} R. Это не гарантированный результат полного потока.")
    rows = [['Пара','W / D / H4: доступно из запрошенного','Исключённых интервалов']]
    for symbol, values in data['initial_context'].items():
        counts=' / '.join(f"{values[tf]['available']}/{values[tf]['requested']}" for tf in ('W','D','H4'))
        rows.append([symbol,counts,str(len(data['sources'][symbol]['excluded_intervals']))])
    table(rows,[85,270,120])
    story.append(paragraph('SHA-256 правил: '+data['strategy_sha256'], 'SmallRu'))
    story.append(paragraph('Версия: '+data['engine_version']+'. Полные хеши архивов, manifests и подготовленных файлов сохранены в results.json; применённые правила - в strategy.json.', 'SmallRu'))
    heading('Методика исполнения')
    labels = {
        'market': 'Рыночное исполнение', 'limit': 'Лимитное исполнение',
        'stop': 'Исполнение стопа', 'target': 'Исполнение цели',
        'costs': 'Издержки', 'gaps': 'Обработка пропусков',
        'drawdown': 'Метод просадки', 'intrabar': 'Внутриминутная модель',
    }
    if execution:
        for key, label in labels.items():
            if key in execution:
                story.append(paragraph(f"{label}: {execution_value(key, execution[key])}.", 'SmallRu'))
        story.append(paragraph(source_label, 'SmallRu'))
    else:
        story.append(paragraph('Подробная методика исполнения и источник котировок не сохранены; восстановить их по сделкам нельзя.', 'SmallRu'))
    story.append(paragraph('Три лучших, три худших и три случайных примера выбираются без повторов. Зерно выборки фиксировано; при малой выборке показываются доступные сделки.', 'SmallRu'))
    story.append(paragraph('Совпадение исторического результата с котировками сервера или брокерским исполнением не утверждается.', 'SmallRu'))
    for index, example in enumerate(data['examples'],1):
        story.append(PageBreak())
        trade=example['trade'];plan=trade.get('basis',{}).get('plan') or {}
        story.append(paragraph(f'РАЗБОР СДЕЛКИ / {index:02d}', 'SmallRu'))
        story.append(paragraph(f"{trade['symbol']}  /  {'Покупка' if trade['side']=='Long' else 'Продажа'}", 'TitleRu'))
        story.append(MetricCards([('Реализованный результат',number(trade['result_r'],2,' R'),RED if trade['result_r'] < 0 else TEAL),('Вход',number(trade['entry']/1e6,5),NAVY),('Стоп / исходный',number(trade['stop']/1e6,5),NAVY)]))
        story.append(paragraph(f"{timestamp(trade['entry_at'])} → {timestamp(trade['exit_at'])} UTC. Правило: {plan.get('rule','-')}.", 'SmallRu'))
        if trade.get('gaps'):
            notice('ПРОПУСКИ ДАННЫХ / сигнал затронут известным пропуском; исход отражает доступные котировки.')
        tf=plan.get('poi_tf','D')
        story.append(candles_chart(example['charts'].get(tf,[]),trade,tf,plan.get('poi')))
        story.append(Spacer(1,6))
        story.append(candles_chart(example['charts'].get('H4',[]),trade,'H4',plan.get('entry_zone')))
        story.append(paragraph('Зоны и основание сохранены в момент сигнала. Последующие свечи показаны только для иллюстрации исхода. Все цены - валюта котировки за единицу базовой валюты.', 'SmallRu'))
    def page(canvas_, doc):
        canvas_.saveState();w,h=A4
        canvas_.setFillColor(colors.HexColor(NAVY));canvas_.rect(0,h-43,w,43,fill=1,stroke=0)
        canvas_.setFillColor(colors.white);canvas_.setFont('ReportBold',9)
        canvas_.drawString(48,h-26,'FOREX   /   RESEARCH')
        canvas_.setFont('Report',7);canvas_.drawRightString(w-48,h-25,'BACKTEST   /   '+str(data['engine_version']))
        canvas_.setStrokeColor(colors.HexColor('#DCE4EA'));canvas_.line(48,39,w-48,39)
        canvas_.setFont('Report',7);canvas_.setFillColor(colors.HexColor(GREY))
        canvas_.drawString(48,25,'FOREX / локальный расчёт / '+data['strategy_sha256'][:12])
        canvas_.drawRightString(w-48,25,f'{doc.page:02d}');canvas_.restoreState()
    doc=SimpleDocTemplate(str(directory/'report.pdf'), pagesize=A4, rightMargin=48,leftMargin=48,topMargin=62,bottomMargin=49,title='Бэктест текущих правил',author='Forex backtest')
    def deterministic_canvas(*args, **kwargs):
        kwargs['invariant']=1
        return canvas.Canvas(*args,**kwargs)
    doc.build(story,onFirstPage=page,onLaterPages=page,canvasmaker=deterministic_canvas)
    return directory/'report.pdf'

if __name__ == '__main__':
    print(render(sys.argv[1]))
