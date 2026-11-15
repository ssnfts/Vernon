//! Speech round trip on the real models: the text-to-speech node speaks a
//! sentence and the speech-to-text node has to write the same words back.

use std::path::PathBuf;

use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use crate::speak::TextToSpeechNode;
use crate::transcribe::SpeechToTextNode;

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn words_ignores_case_and_punctuation() {
    assert_eq!(words("The rear wing, is READY."), ["the", "rear", "wing", "is", "ready"]);
}

#[test]
#[ignore = "needs the Kokoro and Whisper installs, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_speech_round_trip() {
    let root = std::env::var(crate::bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_AI_E2E_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e"))
        .join("roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = CookContext { node_name: "t".into(), work_dir: dir, frame: 0.0 };
    let said = "The rear wing is ready to print. Start the airflow check on the front bumper.";

    let tts = TextToSpeechNode;
    let mut p = Params::from_specs(&tts.info().params);
    p.set("ai_root", ParamValue::Str(root.clone()));
    p.set("text", ParamValue::Str(said.into()));
    p.set("run", ParamValue::Bool(true));
    let out = tts.cook(&ctx, &[], &p).unwrap();
    let Data::File(wav) = &out[0] else { panic!("no audio file") };

    let stt = SpeechToTextNode;
    let mut p = Params::from_specs(&stt.info().params);
    p.set("ai_root", ParamValue::Str(root));
    p.set("audio", ParamValue::Str(wav.to_string_lossy().into()));
    p.set("language", ParamValue::Str("en".into()));
    p.set("run", ParamValue::Bool(true));
    let out = stt.cook(&ctx, &[], &p).unwrap();
    let Data::Json(t) = &out[0] else { panic!("no transcript") };
    let heard = t["text"].as_str().unwrap().to_string();

    let (want, got) = (words(said), words(&heard));
    let hit = want.iter().filter(|w| got.contains(w)).count();
    println!("said:  {said}\nheard: {heard}\nwords matched: {hit}/{}", want.len());
    assert!(hit * 10 >= want.len() * 9, "only {hit} of {} words came back", want.len());
}
