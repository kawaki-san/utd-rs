use ansi_term::{ANSIGenericString, Color::RGB};
use clap::{lazy_static::lazy_static, StructOpt};
use rand::Rng;
use regex::Regex;
use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table, TableBuilder, TableStyle,
};
use tracing::{debug, error, trace};
use utd::{
    args::{PriorityLevel, SortParam},
    data_dir, read_config_file, setup_logger, Config, Configurable, Tags, Task, Tasks,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> Result<()> {
    let args = utd::args::Cli::parse();
    // don't drop guard
    let _guard = setup_logger(args.log.unwrap_or(utd::args::LogLevel::Trace));
    let config = read_config_file(false)?;

    // Adding a new note/task
    if args.note.is_some() || args.add.is_some() {
        if let Err(e) = new_entry(&args) {
            error!("{e}");
        }
    }
    if args.delete.is_some() {
        if let Err(e) = delete_entry(&args.delete.unwrap()) {
            error!("{e}");
        }
    }
    if args.begin.is_some() {
        if let Err(e) = alter_tasks(&args.begin.unwrap(), State::Started) {
            println!("uhmmm: {}", e);
            error!("{e}");
        }
    }
    if args.check.is_some() {
        if let Err(e) = alter_tasks(&args.check.unwrap(), State::Completed) {
            error!("{e}");
        }
    }
    if args.tidy {
        if let Err(e) = remove_completed() {
            error!("{e}");
        }
    }
    if args.re_set_ids {
        if let Err(e) = make_ids_sequential() {
            error!("{e}");
        }
    }
    if let Err(e) = display_content(&config, args.sort.as_ref()) {
        error!("{e}");
    }
    Ok(())
}

fn display_content(config: &Config, args: Option<&SortParam>) -> Result<()> {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let heading_section = sections.title.as_ref();
    let heading_section = heading_section.cloned().unwrap_or_default();
    let tasks = if let Some(sort) = args {
        order_tasks(*sort)?
    } else {
        state_file_contents()?
    };
    let disabled_title = config.disable_title.unwrap_or(false);
    let mut table = TableBuilder::new()
        .style(
            match &*config
                .borders
                .as_ref()
                .cloned()
                .unwrap_or_else(|| "empty".to_string())
                .as_str()
            {
                "elegant" => TableStyle::extended(),
                "extended" => TableStyle::elegant(),
                "empty" => TableStyle::empty(),
                _ => unreachable!(),
            },
        )
        .build();
    if !disabled_title {
        let title_message = draw_titles(&heading_section, &greeting());
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            title_message,
            2,
            Alignment::Center,
        )]))
    }

    let set_tasks: Vec<_> = tasks
        .iter()
        .filter(|f| f.is_task && !f.in_progress)
        .collect();
    let in_progress: Vec<_> = tasks.iter().filter(|f| f.in_progress).collect();
    let notes: Vec<_> = tasks.iter().filter(|f| !f.is_task).collect();
    for (index, i) in set_tasks.iter().enumerate() {
        if i.is_task && !i.in_progress {
            if index == 0 {
                draw_todo_title(config, &tasks, &mut table);
            }
            draw_todo_list(config, i, &mut table);
        }
    }

    for (index, i) in in_progress.iter().enumerate() {
        if i.in_progress {
            if index == 0 {
                draw_progress_title(config, &mut table);
            }
            draw_progress_list(config, i, &mut table);
        }
    }

    for (index, i) in notes.iter().enumerate() {
        if !i.is_task {
            if index == 0 {
                draw_notes_title(config, &mut table);
            }
            draw_notes_list(config, i, &mut table);
        }
    }

    if !tasks.is_empty() {
        println!("{}", table.render());
    }
    Ok(())
}

fn draw_progress_list(config: &Config, task: &Task, table: &mut Table) {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let task_title = format!("{}. {}", task.id, &task.name);
    let res = draw_lists(
        &sections.in_progress.unwrap_or_default(),
        task.is_done,
        task_title,
        &task.priority,
        (
            &task.tags,
            &config.tags.as_ref().cloned().unwrap_or_default(),
        ),
    );
    table.add_row(Row::new(vec![TableCell::new(res); 1]));
}

fn draw_notes_list(config: &Config, task: &Task, table: &mut Table) {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let task_title = format!("{}. {}", task.id, &task.name);
    let res = draw_lists(
        &sections.notes.unwrap_or_default(),
        task.is_done,
        task_title,
        &task.priority,
        (
            &task.tags,
            &config.tags.as_ref().cloned().unwrap_or_default(),
        ),
    );
    table.add_row(Row::new(vec![TableCell::new(res); 1]));
}
fn draw_todo_list(config: &Config, task: &Task, table: &mut Table) {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let task_title = format!("{}. {}", task.id, &task.name);
    let res = draw_lists(
        &sections.todo.unwrap_or_default(),
        task.is_done,
        task_title,
        &task.priority,
        (
            &task.tags,
            &config.tags.as_ref().cloned().unwrap_or_default(),
        ),
    );
    table.add_row(Row::new(vec![TableCell::new(res); 1]));
}

fn draw_lists<'a>(
    config: &'a impl Configurable,
    completed: bool,
    value: String,
    priority: &'a str,
    tags: (&str, &Tags),
) -> String {
    let (tag_text, tags) = tags;
    let mut padding = String::default();
    for _ in 0..config.indent_spaces() + 2 {
        padding.push(' ');
    }
    let value = if config.entry_icon_suffix() {
        if completed {
            format!("{}{}", value, config.completed_icon())
        } else {
            format!("{}{}", value, config.entry_icon())
        }
    } else if completed {
        format!("{}{}", config.completed_icon(), value)
    } else {
        format!("{}{}", config.entry_icon(), value)
    };
    /***********
     ***/

    let tag_text = if tags.icon_suffix.unwrap_or(false) {
        if !tag_text.is_empty() {
            format!("{}{}", tag_text, tags.icon())
        } else {
            tag_text.to_owned()
        }
    } else if !tag_text.is_empty() {
        format!("{}{}", tags.icon(), tag_text)
    } else {
        tag_text.to_owned()
    };
    /************************/
    let hex_title = match completed {
        false => match &*priority {
            "low" => hex_to_rgb(config.colour_low()),
            "normal" => hex_to_rgb(config.colour_normal()),
            "high" => hex_to_rgb(config.colour_high()),
            _ => unreachable!(),
        },
        true => hex_to_rgb(config.colour_completed()),
    };

    let heading = if config.dim_completed() {
        if config.entry_italic() && config.entry_bold() && completed {
            RGB(hex_title.0, hex_title.1, hex_title.2)
                .italic()
                .strikethrough()
                .dimmed()
                .bold()
        } else if config.entry_italic() && !config.entry_bold() && !completed {
            RGB(hex_title.0, hex_title.1, hex_title.2).italic()
        } else if !config.entry_italic() && config.entry_bold() && !completed {
            RGB(hex_title.0, hex_title.1, hex_title.2).bold()
        } else if !config.entry_italic() && !config.entry_bold() && completed {
            RGB(hex_title.0, hex_title.1, hex_title.2)
                .strikethrough()
                .dimmed()
        } else if !config.entry_italic() && config.entry_bold() && completed {
            RGB(hex_title.0, hex_title.1, hex_title.2)
                .strikethrough()
                .dimmed()
                .bold()
        } else if config.entry_italic() && config.entry_bold() && completed {
            RGB(hex_title.0, hex_title.1, hex_title.2)
                .italic()
                .bold()
                .dimmed()
                .strikethrough()
        } else if config.entry_italic() && !config.entry_bold() && completed {
            RGB(hex_title.0, hex_title.1, hex_title.2)
                .italic()
                .dimmed()
                .strikethrough()
        } else {
            RGB(hex_title.0, hex_title.1, hex_title.2).normal()
        }
    } else if config.entry_italic() && config.entry_bold() && completed {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .italic()
            .strikethrough()
            .bold()
    } else if config.entry_italic() && !config.entry_bold() && !completed {
        RGB(hex_title.0, hex_title.1, hex_title.2).italic()
    } else if !config.entry_italic() && config.entry_bold() && !completed {
        RGB(hex_title.0, hex_title.1, hex_title.2).bold()
    } else if !config.entry_italic() && !config.entry_bold() && completed {
        RGB(hex_title.0, hex_title.1, hex_title.2).strikethrough()
    } else if !config.entry_italic() && config.entry_bold() && completed {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .strikethrough()
            .bold()
    } else if config.entry_italic() && config.entry_bold() && completed {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .italic()
            .bold()
            .strikethrough()
    } else if config.entry_italic() && !config.entry_bold() && completed {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .italic()
            .strikethrough()
    } else {
        RGB(hex_title.0, hex_title.1, hex_title.2).normal()
    };
    let vals = heading.paint(value);
    let res = format!("{padding}{vals}");
    let hex_title_tag = hex_to_rgb(tags.colour());
    let tag = if tags.italic() && tags.bold() && tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2)
            .italic()
            .underline()
            .bold()
    } else if tags.italic() && !tags.bold() && !tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2).italic()
    } else if !tags.italic() && tags.bold() && !tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2).bold()
    } else if !tags.italic.unwrap() && !tags.bold() && tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2).underline()
    } else if !tags.italic() && tags.bold() && tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2)
            .underline()
            .bold()
    } else if tags.italic() && tags.bold() && !tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2)
            .italic()
            .bold()
    } else if tags.italic() && !tags.bold() && tags.underline() {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2)
            .italic()
            .underline()
    } else {
        RGB(hex_title_tag.0, hex_title_tag.1, hex_title_tag.2).normal()
    };
    let other = tag.paint(tag_text);
    format!("{res} {other}")
}

fn draw_titles(title: &impl Configurable, value: impl AsRef<str>) -> ANSIGenericString<str> {
    let hex_title = hex_to_rgb(title.title_colour());
    let heading = if title.title_italic() && title.title_bold() && title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .italic()
            .underline()
            .bold()
    } else if title.title_italic() && !title.title_bold() && !title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2).italic()
    } else if !title.title_italic() && title.title_bold() && !title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2).bold()
    } else if !title.title_italic() && !title.title_bold() && title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2).underline()
    } else if !title.title_italic() && title.title_bold() && title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .underline()
            .bold()
    } else if title.title_italic() && title.title_bold() && !title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2).italic().bold()
    } else if title.title_italic() && !title.title_bold() && title.title_underline() {
        RGB(hex_title.0, hex_title.1, hex_title.2)
            .italic()
            .underline()
    } else {
        RGB(hex_title.0, hex_title.1, hex_title.2).normal()
    };
    heading.paint(if !title.title_icon_suffix() {
        format!("{}{}", title.title_icon(), value.as_ref())
    } else {
        format!("{}{}", value.as_ref(), title.title_icon())
    })
}

fn draw_todo_title(config: &Config, tasks: &Tasks, table: &mut Table) {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let task_count = tasks.iter().filter(|f| f.is_task).count();
    let completed_count = tasks.iter().filter(|f| f.is_task && f.is_done).count();
    let heading_to_do = format!("to-do [{}/{}]", completed_count, task_count);
    let heading_section = sections.todo.as_ref();
    let heading_section = heading_section.cloned().unwrap_or_default();
    let heading_to_do = draw_titles(&heading_section, &heading_to_do);
    let heading = sections.todo.unwrap_or_default();
    let heading = heading.indent_spaces();
    let mut padding = String::default();
    for _ in 0..heading {
        padding.push(' ')
    }
    table.add_row(Row::new(vec![
        TableCell::new(format!(
            "{}{}",
            padding, heading_to_do
        ));
        1
    ]));
}

fn draw_progress_title(config: &Config, table: &mut Table) {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let heading = String::from("in progress");
    let heading_section = sections.in_progress.as_ref();
    let heading_section = heading_section.cloned().unwrap_or_default();
    let heading_to_do = draw_titles(&heading_section, &heading);
    let heading = sections.in_progress.unwrap_or_default();
    let heading = heading.indent_spaces();
    let mut padding = String::default();
    for _ in 0..heading {
        padding.push(' ')
    }
    table.add_row(Row::new(vec![
        TableCell::new(format!(
            "{}{}",
            padding, heading_to_do
        ));
        1
    ]));
}

fn draw_notes_title(config: &Config, table: &mut Table) {
    let section = config.sections.as_ref();
    let sections = section.cloned().unwrap_or_default();
    let heading = String::from("notes");
    let heading_section = sections.notes.as_ref();
    let heading_section = heading_section.cloned().unwrap_or_default();
    let heading_to_do = draw_titles(&heading_section, &heading);
    let heading = sections.notes.unwrap_or_default();
    let heading = heading.indent_spaces();
    let mut padding = String::default();
    for _ in 0..heading {
        padding.push(' ')
    }
    table.add_row(Row::new(vec![
        TableCell::new(format!(
            "{}{}",
            padding, heading_to_do
        ));
        1
    ]));
}

fn order_tasks(sort: utd::args::SortParam) -> Result<Tasks> {
    let mut tasks = state_file_contents()?;
    match sort {
        utd::args::SortParam::Age => tasks.sort_unstable_by_key(|f| f.timestamp()),
        utd::args::SortParam::Priority => {
            tasks.sort_unstable_by_key(|f| f.priority_score());
            tasks.reverse();
        }
    }
    Ok(tasks)
}

fn make_ids_sequential() -> Result<()> {
    let tasks = state_file_contents()?;
    let mut c_tasks = tasks.clone();
    for (index, _task) in tasks.into_iter().enumerate() {
        let t = c_tasks.get_mut(index).unwrap();
        t.id = (index + 1) as i64;
    }
    update_file(&c_tasks)?;
    Ok(())
}

fn remove_completed() -> Result<()> {
    let mut tasks = state_file_contents()?;
    tasks = tasks
        .iter()
        .filter_map(|f| {
            if f.is_done.to_string() != true.to_string() {
                Some(f.to_owned())
            } else {
                None
            }
        })
        .collect();
    update_file(&tasks)?;
    Ok(())
}

enum State {
    Started,
    Completed,
}

fn alter_tasks(ids: &[String], state: State) -> Result<()> {
    let mut tasks = state_file_contents()?;
    for i in ids.iter() {
        let i: usize = i.parse()?;
        let vals = tasks
            .clone()
            .into_iter()
            .map(|mut f| {
                if f.id as usize == i {
                    match state {
                        State::Started => {
                            f.in_progress = !f.in_progress;
                            f.is_done = false;

                            debug!("starting task {}: {}", i, f.name);
                        }
                        State::Completed => {
                            f.in_progress = false;
                            f.is_done = true;
                            debug!("completing task {}: {}", i, f.name);
                        }
                    }
                }
                f
            })
            .collect();
        tasks = vals;
    }
    update_file(&tasks)?;
    debug!("{} tasks updated - ok", ids.len());
    Ok(())
}

fn delete_entry(ids: &[String]) -> Result<()> {
    let mut tasks = state_file_contents()?;
    for i in ids.iter() {
        let num: i64 = i.parse()?;
        tasks = tasks
            .iter()
            .filter_map(|f| {
                if f.id != num {
                    Some(f.to_owned())
                } else {
                    None
                }
            })
            .collect();
    }
    update_file(&tasks)?;
    debug!("{} tasks deleted - ok", ids.len());
    Ok(())
}

fn new_entry(args: &utd::args::Cli) -> Result<()> {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(@.\w+)").unwrap();
    }
    fn timestamp() -> std::time::Duration {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time is going backwards")
    }
    let entry_adder = |list: &[String],
                       is_task: bool,
                       file: &mut File,
                       priority: &mut VecDeque<&PriorityLevel>|
     -> Result<()> {
        let mut tasks: Tasks = state_file_contents()?;
        {
            // Check if file has data in it
            if !tasks.is_empty() {
                trace!("found {} existing tasks", tasks.len());
            } else {
                trace!("found no existing tasks");
            }
        }
        let mut entries = Vec::with_capacity(list.len());
        let mut len = match tasks.iter().max_by_key(|f| f.id) {
            Some(task) => task.id,
            None => 0,
        };
        for entry_name in list.iter() {
            let tags: Vec<_> = RE.find_iter(entry_name).map(|f| f.as_str()).collect();
            let title = RE.replace_all(entry_name, " ");
            len += 1;
            let task = Task::new(
                &title,
                &tags.join(" "),
                is_task,
                len,
                *priority.pop_front().unwrap_or(&PriorityLevel::Normal),
                timestamp().as_nanos(),
            );
            entries.push(task);
        }
        tasks.append(&mut entries);
        write_to_file(file, &tasks);
        Ok(())
    };
    let mut path = data_dir();
    path.push(".utd.json");
    // if note is some, iterate and add notes
    let default_vec = &vec![
        PriorityLevel::Normal;
        match args.add.as_ref() {
            Some(tasks) => tasks.len(),
            // Safe to unwrap since we are sure one of them is some
            None => args.note.as_ref().unwrap().len(),
        }
    ];

    let mut vd = VecDeque::from_iter(args.priority.as_ref().unwrap_or(default_vec));
    if let Some(ref tasks) = args.add {
        entry_adder(tasks, true, &mut state_file(&path, false, true)?, &mut vd)?;
    }
    if let Some(ref notes) = args.note {
        entry_adder(notes, false, &mut state_file(&path, false, true)?, &mut vd)?;
    }
    Ok(())
}

fn state_file(path: &PathBuf, read: bool, write: bool) -> Result<File> {
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .write(write)
        .read(read)
        .open(&path)?)
}

fn write_to_file(file: &mut File, tasks: &Tasks) {
    writeln!(file, "{}", serde_json::to_string_pretty(tasks).unwrap()).unwrap();
    trace!("tasks updated");
}

fn state_file_contents() -> Result<Tasks> {
    let mut path = data_dir();
    path.push(".utd.json");
    let read_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(&path)
        .unwrap();
    let mut buf_reader = std::io::BufReader::new(read_file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;
    // empty list
    if contents.is_empty() {
        contents.push_str("[]");
    }
    let tasks: Tasks = serde_json::from_str(&contents)?;
    Ok(tasks)
}

fn update_file(tasks: &Tasks) -> Result<()> {
    let mut path = data_dir();
    path.push(".temp");
    let mut temp = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&path)?;
    write_to_file(&mut temp, tasks);
    let mut original = data_dir();
    original.push(".utd.json");
    std::fs::rename(path, original)?;
    Ok(())
}

fn greeting() -> String {
    let greetings = || -> Vec<String> {
        vec![
            "Here's is your board",
            "Remember...",
            "Let's get things done",
            "Focus",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    };

    let greetings = greetings();
    let num = rand::thread_rng().gen_range(0..greetings.len());
    greetings.get(num).unwrap().to_owned()
}

fn hex_to_rgb(hex_colour: &str) -> (u8, u8, u8) {
    let first = u8::from_str_radix(&hex_colour[1..3], 16).unwrap();
    let second = u8::from_str_radix(&hex_colour[3..5], 16).unwrap();
    let third = u8::from_str_radix(&hex_colour[5..7], 16).unwrap();
    (first, second, third)
}
