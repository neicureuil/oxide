import AppKit
import SwiftUI

/// Settings › Langue: automatic spelling correction in the composer, the same choice as Edit › Spelling and Grammar.
struct SpellingSettingsSection: View {
    @AppStorage(ComposerTextView.automaticSpellingCorrectionKey) private var stored: Bool?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Toggle(isOn: Binding(
                get: { stored ?? NSSpellChecker.isAutomaticSpellingCorrectionEnabled },
                set: { stored = $0 }
            )) {
                Text(L("ui.spelling_settings.correct_spelling_automatically")).font(.figtree(14, .semibold))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toggleStyle(.switch)
            Muted(L("ui.spelling_settings.correct_spelling_automatically_detail"), size: 12)
        }
        .card(padding: 18)
    }
}
