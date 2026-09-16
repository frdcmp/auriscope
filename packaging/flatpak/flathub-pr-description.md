<!-- ⚠️⚠️  Submission pull request MUST be made against the `new-pr` base branch ⚠️⚠️  -->

### Please confirm your submission meets all the criteria

* [x] Please describe the application briefly.

Auriscope is a read-only audio player and analyser: waveform, whole-file spectrogram with optional time-frequency reassignment, realtime spectrum, EBU R128 loudness and level measurements, and a RIFF/WAVE header inspector. Built in Rust with egui/wgpu under the GPL-3.0-or-later license.

* [x] Please attach a video showcasing the application on Linux using the Flatpak.

https://github.com/user-attachments/assets/b17a773e-e7f7-446e-9579-ebc0baabdd80

* [x] The Flatpak ID follows all the rules listed in the [Application ID requirements][appid].

`io.github.frdcmp.Auriscope`

* [x] I have read and followed all the [Submission requirements][reqs] and the [Submission guide][reqs2] and I agree to them.
  * [x] The application has a meaningful development history, evidence of real-world use, and a clear commitment to ongoing maintenance, as required by the [development history requirements][history].

    > **History:** The repository went public on 16 September 2026 with v0.1.0 as the first tagged release.
    >
    > **Real-world use:** I built this tool for my own audio work at my company's audio technology and data collection team, which operates 8 professional studios. It serves as a lightweight utility to instantly inspect audio clips (such as checking the noise floor) without the overhead or slow startup times of a full DAW. I tested every feature as it was built, and I have run it on real production audio from our team's projects.
    >
    > **Maintenance:** While the tool is utilized by our studio team, I am the sole author and maintainer. I am fully committed to maintaining this application long-term, upgrading its Flatpak runtime dependencies, and fixing upstream bugs.

  * [x] I have disclosed any AI-generated material included in the application or its Flathub packaging, as required by the [Generative AI policy][ai].

    **Affected parts and approximate extent:** Most of the Rust source in `src/`, plus the Flatpak manifest (`io.github.frdcmp.Auriscope.yml`), the cargo-sources generation script (`gen-cargo-sources.py`), the desktop entry, the AppStream metainfo, the Arch PKGBUILDs and the release workflow, were AI-generated under my direction. I reviewed and tested all of it.

  * [x] I have not used AI tools or agents to generate or automate this submission pull request or its review interactions.

* [x] I am an author/developer/upstream contributor to the project.

  **Link:** https://github.com/frdcmp/auriscope

<!-- ⚠️⚠️  Please DO NOT modify anything below this line ⚠️⚠️  -->

[appid]: https://docs.flathub.org/docs/for-app-authors/requirements#application-id
[reqs]: https://docs.flathub.org/docs/for-app-authors/requirements
[reqs2]: https://docs.flathub.org/docs/for-app-authors/submission
[history]: https://docs.flathub.org/docs/for-app-authors/requirements#insufficient-development-history
[ai]: https://docs.flathub.org/docs/for-app-authors/requirements#generative-ai-policy
