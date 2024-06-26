use std::error::Error;
use crate::messenger::common::parse_attributes;
use crate::messenger::{common, Data, FileType, Image, Message, ArchiveDetails, Text, MessengerArchive};
use chrono::{NaiveDateTime, NaiveTime, Timelike};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use xml::attribute::OwnedAttribute;
use xml::EventReader;
use xml::reader::XmlEvent;


pub struct MessengerPlusParser<'a> {
    details: ArchiveDetails,
    reader: EventReader<BufReader<File>>,
    parents: (String, Vec<OwnedAttribute>),
    session: MsgPlusSession,
    directory: &'a Path,
    first_message: bool,
}

#[derive(Default)]
struct MsgPlusSession {
    date: NaiveDateTime,
    id: String,
    owner: String,
    recipient: String,
    message_style: String,
}


impl<'a> MessengerPlusParser<'a> {
    pub fn new(path: &'a str) -> Result<Self, Box<dyn Error>> {
        let path_t = Path::new(path);
        Ok(MessengerPlusParser {
            details: ArchiveDetails {
                recipient_id: path_t
                    .file_stem()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or_default()
                    .to_string(),
                file_type: FileType::MessengerPlus,
                ..ArchiveDetails::default()
            },
            reader:  common::get_parser(path)?,
            parents: ("".to_string(), vec![]),
            session: MsgPlusSession::default(),
            directory: path_t
                .parent().ok_or("The file must be somewhere in a directory")?,
            first_message: true,
        })
    }

    fn parse_node(&mut self, name: &str, attributes: &Vec<OwnedAttribute>, message: &mut Message) -> Result<(), Box<dyn Error>> {
        let attributes = parse_attributes(attributes);
        match name {
            "div" => {
                if self.parents.0.ends_with("html.body")
                    && attributes
                    .iter()
                    .any(|(attr, val)| attr.eq(&"class") && val.eq(&"mplsession"))
                {
                    if let Some(id) = attributes.get("id") {
                        self.session.id = id.to_string();
                        self.session.date = NaiveDateTime::parse_from_str(id, "Session_%Y-%m-%dT%H-%M-%S")?;
                        if self.details.first_session_id.is_empty() {
                            self.details.first_session_id = id.to_string();
                        }
                    }
                }
            }
            "td" => {
                if self.parents.0.ends_with("html.body.div.table.tbody.tr")
                    && attributes.iter().any(|(attr, ..)| attr.eq(&"style"))
                {
                    if let Some(style) = attributes.get("style") {
                        self.session.message_style =
                            html_escape::decode_html_entities(style).trim().to_string()
                    }
                }
            }
            "tr" => {
                if self.parents.0.ends_with("html.body.div.table.tbody") {
                    message.session_id = self.session.id.to_string();
                    if attributes
                        .iter()
                        .any(|(attr, val)| attr.eq(&"class") && val.eq(&"msgplus"))
                    {
                        message.data = vec![Data::System("".to_string())];
                    }
                }
            }
            "img" => {
                if self.parents.0.ends_with("html.body.div.table.tbody.tr.td")
                    && attributes.iter().any(|(attr, ..)| attr.eq(&"src"))
                {
                    let mut img = Image::default();

                    if let Some(alt) = attributes.get("alt") {
                        img.alt = alt.trim().to_string();
                    }
                    if let Some(src) = attributes.get("src") {
                        img.src = src.trim().to_string();
                        let mut buffer = Vec::new();
                        File::open(self.directory.join(src))?.read_to_end(&mut buffer)?;
                        img.content = buffer;
                    }
                    message.data.push(Data::Image(img));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn parse_text(&mut self, data: &str, message: &mut Message) -> Result<(), Box<dyn Error>> {
        match self.parents.0.as_str() {
            ".html.body.div.ul.li" => {
                let attributes = parse_attributes(&self.parents.1);
                if attributes
                    .iter()
                    .any(|(attr, val)| attr.eq(&"class") && val.eq(&"in"))
                {
                    self.session.owner = data.trim().to_string();
                } else {
                    self.session.recipient = data.trim().to_string();
                }
            }
            ".html.body.div.table.tbody.tr.th.span" => {
                if self.first_message {
                    let datetime = NaiveDateTime::new(
                        self.session.date.date(),
                        NaiveTime::parse_from_str(
                            format!("{}:{}", data, self.session.date.second()).as_str(),
                            "(%H:%M):%S",
                        )?,
                    );
                    message.datetime = datetime.format("%Y-%m-%dT%H:%M:%S").to_string();
                    self.first_message = false;
                } else {
                    let datetime = NaiveDateTime::new(
                        self.session.date.date(),
                        NaiveTime::parse_from_str(data, "(%H:%M)")?,
                    );
                    message.datetime = datetime.format("%Y-%m-%dT%H:%M").to_string();
                };
            }
            ".html.body.div.table.tbody.tr.th" => {
                if data.matches(&self.session.owner).count() > 0 {
                    message.sender_friendly_name = self.session.owner.to_string();
                    message.receiver_friendly_name = self.session.recipient.to_string();
                } else {
                    message.sender_friendly_name = self.session.recipient.to_string();
                    message.receiver_friendly_name = self.session.owner.to_string();
                }
            }
            ".html.body.div.table.tbody.tr.td" => {
                let attributes = parse_attributes(&self.parents.1);
                if let Some(Data::System(_)) = message.data.first() {
                    message.data.push(Data::System(data.to_string()));
                    message.data.swap_remove(0);
                } else {
                    let mut txt = Text {
                        content: data.to_string(),
                        ..Text::default()
                    };
                    match attributes.get("style") {
                        None => {
                            txt.style = self.session.message_style.clone();
                        }
                        Some(style) => {
                            txt.style = style.trim().to_string();
                        }
                    };
                    message.data.push(Data::Text(txt));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl<'a> Iterator for MessengerPlusParser<'a>  {
    type Item = Result<Message, Box<dyn Error>>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut message = Message::default();
        loop {
            let e = self.reader.next();
            match e {
                Ok(XmlEvent::StartElement {
                       name, attributes, ..
                   }) => {
                    let res = self.parse_node(&name.local_name, &attributes, &mut message);
                    if let Err(e) = res { return Some(Err(e)); }
                    self.parents.0 = format!("{}.{}", self.parents.0, name.local_name);
                    self.parents.1 = attributes;
                }
                Ok(XmlEvent::Characters(data)) => {
                    let res = self.parse_text(&data, &mut message);
                    if let Err(e) = res { return Some(Err(e)); }
                }
                Ok(XmlEvent::EndElement { name }) => {
                    let new_selector = match self.parents.0.rfind('.') {
                        Some(pos) => &self.parents.0[0..pos],
                        None => "",
                    };
                    self.parents.0 = new_selector.to_string();
                    if name.local_name.eq("tr") && self.parents.0.ends_with("html.body.div.table.tbody") {
                        return Some(Ok(message));
                    }
                }
                Ok(XmlEvent::EndDocument) => {
                    self.details.last_session_id = self.session.id.clone();
                    return None;
                }
                Err(e) => { return Some(Err(Box::new(e))) },
                _ => {}
            }
        }
    }
}

impl<'a> MessengerArchive for MessengerPlusParser<'a> {
    fn details(&self) -> Option<&ArchiveDetails> {
        if self.details.last_session_id.is_empty() {
            None
        } else {
            Some(&self.details)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Read;

    #[test]
    fn parse_sample_file() {
        let path = "test/alice@example.com.html";
        let mut parser = MessengerPlusParser::new(path).unwrap();

        let mut f = File::open("test/Images/MsgPlus_Img0663.png").unwrap();
        let mut buffer = Vec::new();
        f.read_to_end(&mut buffer).unwrap();

        let details = ArchiveDetails {
            file_type: FileType::MessengerPlus,
            recipient_id: "alice@example.com".to_string(),
            first_session_id: "Session_2009-08-05T19-30-21".to_string(),
            last_session_id: "Session_2009-08-05T19-30-21".to_string(),
        };
        let messages = vec![
            Message {
                datetime: "2009-08-05T19:30:21".to_string(),
                timezone_offset: None,
                session_id: "Session_2009-08-05T19-30-21".to_string(),
                sender_friendly_name: "Bob".to_string(),
                receiver_friendly_name: "Alice".to_string(),
                data: vec![
                    Data::Text(Text {
                        style: "font-family:\"Courier New\";color:#004000;".to_string(),
                        content: "Hello Alice!".to_string(),
                    }),
                    Data::Text(Text {
                        style: "font-family:\"Courier New\";color:#004000;".to_string(),
                        content: "How are you?".to_string(),
                    }),
                ],
            },
            Message {
                datetime: "2009-08-05T19:30".to_string(),
                timezone_offset: None,
                session_id: "Session_2009-08-05T19-30-21".to_string(),
                sender_friendly_name: "Alice".to_string(),
                receiver_friendly_name: "Bob".to_string(),
                data: vec![
                    Data::Text(Text {
                        style: "font-family:\"Segoe UI\";".to_string(),
                        content: "I'm fine, thank you!".to_string(),
                    }),
                    Data::Text(Text {
                        style: "font-family:\"Segoe UI\";".to_string(),
                        content: "What about you?".to_string(),
                    }),
                    Data::Text(Text {
                        style: "font-family:\"Segoe UI\";".to_string(),
                        content: "Have you called John about this weekend?".to_string(),
                    }),
                ],
            },
            Message {
                datetime: "2009-08-05T19:31".to_string(),
                timezone_offset: None,
                session_id: "Session_2009-08-05T19-30-21".to_string(),
                sender_friendly_name: "Bob".to_string(),
                receiver_friendly_name: "Alice".to_string(),
                data: vec![
                    Data::Text(Text {
                        style: "font-family:\"Courier New\";color:#004000;".to_string(),
                        content: "Yes!".to_string(),
                    }),
                    Data::Text(Text {
                        style: "font-family:\"Courier New\";color:#004000;".to_string(),
                        content: "He should have called you...".to_string(),
                    }),
                ],
            },
            Message {
                datetime: "2009-08-05T19:31".to_string(),
                timezone_offset: None,
                session_id: "Session_2009-08-05T19-30-21".to_string(),
                sender_friendly_name: "Alice".to_string(),
                receiver_friendly_name: "Bob".to_string(),
                data: vec![Data::Text(Text {
                    style: "font-family:\"Segoe UI\";".to_string(),
                    content: "He didn't!".to_string(),
                })],
            },
            Message {
                datetime: "2009-08-05T19:35".to_string(),
                timezone_offset: None,
                session_id: "Session_2009-08-05T19-30-21".to_string(),
                sender_friendly_name: "Bob".to_string(),
                receiver_friendly_name: "Alice".to_string(),
                data: vec![
                    Data::Image(Image {
                        src: "./Images/MsgPlus_Img0663.png".to_string(),
                        alt: ":)".to_string(),
                        content: buffer,
                    }),
                    Data::Text(Text {
                        style: "font-family:\"Courier New\";color:#004000;".to_string(),
                        content: "Maybe you can call him?".to_string(),
                    }),
                ],
            },
            Message {
                datetime: "2009-08-05T19:44".to_string(),
                timezone_offset: None,
                session_id: "Session_2009-08-05T19-30-21".to_string(),
                sender_friendly_name: "".to_string(),
                receiver_friendly_name: "".to_string(),
                data: vec![Data::System("Alice is now offline".to_string())],
            },
        ];
        assert_eq!(parser.next().unwrap().unwrap(), messages[0]);
        assert_eq!(parser.next().unwrap().unwrap(), messages[1]);
        assert_eq!(parser.next().unwrap().unwrap(), messages[2]);
        assert_eq!(parser.next().unwrap().unwrap(), messages[3]);
        assert_eq!(parser.next().unwrap().unwrap(), messages[4]);
        assert_eq!(parser.next().unwrap().unwrap(), messages[5]);
        assert!(parser.next().is_none());
        assert_eq!(parser.details(), Some(&details));
    }
}
