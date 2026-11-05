//! DecentSampler fuzz seeds shared by `ds_fixtures.rs` and `fuzz.rs`: malformed presets and
//! well-formed shapes of the F-DS fixtures (they play `a.wav`).

#![allow(dead_code)]

/// Presets the reader must refuse (not well-formed, or not a DecentSampler preset).
pub const MALFORMED: &[&str] = &[
    "",
    "<DecentSampler>",
    "<DecentSampler><groups></group></DecentSampler>",
    "<DecentSampler><groups><sample path='a.wav' path='b.wav'/></groups></DecentSampler>",
    "<DecentSampler><groups/></DecentSampler><extra/>",
    "<NotDecent><groups/></NotDecent>",
    "<DecentSampler><ui/></DecentSampler>",
    "<DecentSampler><groups><group><sample path=\"&bogus;\"/></group></groups></DecentSampler>",
];

/// Well-formed presets for mutation.
pub const VALID: &[&str] = &[
    r#"<?xml version="1.0" encoding="UTF-8"?>
<DecentSampler minVersion="1.0.0">
  <groups volume="-3dB" globalPan="10" attack="0.01" release="0.3">
    <group name="Main" groupVolume="0.5" seqMode="round_robin" tags="body">
      <sample path="a.wav" rootNote="60" loNote="48" hiNote="72" loVel="0" hiVel="100" seqPosition="1"/>
      <sample path="a.wav" rootNote="60" loNote="48" hiNote="72" seqPosition="2" loCC1="0" hiCC1="63" trigger="release" releaseTriggerDecay="3dB"/>
    </group>
  </groups>
</DecentSampler>"#,
    r#"<DecentSampler>
  <ui width="812" height="375"><tab name="main"><labeled-knob x="10" y="10" label="Tone" minValue="0" maxValue="1" value="0.5"><binding type="effect" level="instrument" position="0" parameter="FX_FILTER_FREQUENCY" translation="table" translationTable="0,60;1,22000"/></labeled-knob></tab></ui>
  <groups><group enabled="false" name="Long"><sample path="a.wav" rootNote="60" loopStart="100" loopEnd="1000" loopCrossfade="50" loopCrossfadeMode="linear" loopDuringRelease="false"/></group>
  <group name="Short" ampEnvEnabled="false"><sample path="a.wav" rootNote="60" tags="hat" silencedByTags="hat,open" silencingMode="normal"/></group></groups>
  <effects><effect type="lowpass" frequency="3000" resonance="1.5"/><effect type="reverb" wetLevel="0.3"/><effect type="wave_folder"/></effects>
  <midi><note note="24"><binding type="amp" level="group" position="0" parameter="ENABLED" translation="fixed_value" translationValue="true"/><binding type="amp" level="group" position="1" parameter="ENABLED" translation="fixed_value" translationValue="false"/></note><cc number="1"><binding type="amp" level="instrument" parameter="AMP_VOLUME" translationOutputMin="0.5" translationOutputMax="1"/></cc></midi>
  <modulators><lfo shape="sine" frequency="5" modAmount="0.5" scope="voice"><binding type="amp" level="group" groupIndex="1" parameter="GROUP_TUNING" translationOutputMin="-1" translationOutputMax="1"/></lfo><random mode="note_on"><binding type="amp" level="instrument" parameter="PAN" translationOutputMin="-20" translationOutputMax="20"/></random></modulators>
  <tags><tag name="hat" polyphony="1" volume="0.8"/></tags>
  <buses><bus busVolume="0.7"><effects><effect type="delay" delayTime="0.25"/></effects></bus></buses>
  <noteSequences><sequence name="s" length="4"><note position="0" note="60" length="1"/></sequence></noteSequences>
</DecentSampler>"#,
];
