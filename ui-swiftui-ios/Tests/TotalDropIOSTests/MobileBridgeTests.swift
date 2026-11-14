// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import Testing
@testable import TotalDropIOS

@Suite("TotalDropIOS Universal Links & VoiceOver")
struct MobileBridgeTests {
    @Test("Parses Universal Link fragment code")
    func universalLinkFragment() throws {
        let url = try #require(URL(string: "https://totaldrop.app/r#7-harbor-rocket"))
        #expect(IOSAppModel.parseLinkCode(url) == "7-harbor-rocket")
    }

    @Test("Produces VoiceOver comma-paused code string")
    func spokenCode() {
        #expect(IOSAppModel.spokenVoiceOverCode("7-harbor-rocket") == "7, harbor, rocket")
    }
}
