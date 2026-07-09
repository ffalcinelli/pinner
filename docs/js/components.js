/**
 * Reusable Components for the Pinner Documentation Site
 */

async function fetchLatestVersion() {
    try {
        const response = await fetch('https://api.github.com/repos/ffalcinelli/pinner/releases/latest');
        if (response.ok) {
            const data = await response.json();
            if (data && data.tag_name) {
                return data.tag_name;
            }
        }
    } catch (e) {
        console.warn('Failed to fetch latest version from GitHub, using fallback.', e);
    }
    return 'v0.0.12'; // Fallback
}

class PinnerNavbar extends HTMLElement {
    async connectedCallback() {
        const activePage = this.getAttribute('active-page') || '';
        
        const getLinkClass = (page) => {
            return activePage === page 
                ? 'text-sky-400 font-bold flex items-center transition-colors' 
                : 'hover:text-sky-400 text-gray-400 flex items-center transition-colors';
        };

        this.innerHTML = `
        <nav class="flex items-center justify-between px-4 md:px-8 py-6 max-w-7xl mx-auto relative z-10">
            <div class="flex items-center space-x-2">
                <a href="index.html" class="flex items-center space-x-2 group">
                    <i class="fas fa-thumbtack text-sky-400 text-2xl group-hover:rotate-45 transition-transform"></i>
                    <span class="text-2xl font-bold tracking-tight text-white">Pinner</span>
                </a>
            </div>
            <div class="flex items-center space-x-4 md:space-x-6 text-sm font-medium">
                <a href="getting-started.html" class="${getLinkClass('getting-started')}" title="Getting Started">
                    <i class="fas fa-rocket md:mr-2"></i><span class="hidden md:inline">Getting Started</span>
                </a>
                <a href="configuration.html" class="${getLinkClass('configuration')}" title="Configuration">
                    <i class="fas fa-cog md:mr-2"></i><span class="hidden md:inline">Configuration</span>
                </a>
                <a href="https://github.com/ffalcinelli/pinner" class="hover:text-sky-400 text-gray-400 flex items-center transition-colors" title="GitHub">
                    <i class="fab fa-github md:mr-2"></i><span class="hidden md:inline">GitHub</span>
                </a>
                <a href="https://docs.rs/pinner" class="hover:text-sky-400 text-gray-400 flex items-center transition-colors" title="API Docs">
                    <i class="fas fa-book md:mr-2"></i><span class="hidden md:inline">API Docs</span>
                </a>
            </div>
        </nav>`;

        const version = await fetchLatestVersion();
        document.querySelectorAll('.pinner-version-badge').forEach(el => {
            el.textContent = version;
        });
        document.querySelectorAll('.pinner-version-text').forEach(el => {
            el.textContent = `Latest Release: ${version}`;
        });
    }
}
customElements.define('pinner-navbar', PinnerNavbar);

class PinnerFooter extends HTMLElement {
    connectedCallback() {
        this.innerHTML = `
        <footer class="py-16 text-center text-gray-500 border-t border-gray-800/20 mt-16 max-w-7xl mx-auto px-4 relative z-10">
            <div class="flex items-center justify-center space-x-4 mb-6">
                <a href="https://github.com/ffalcinelli/pinner" class="hover:text-white transition-colors" title="GitHub"><i class="fab fa-github text-xl"></i></a>
                <div class="h-4 w-px bg-gray-800"></div>
                <a href="https://docs.rs/pinner" class="hover:text-white transition-colors text-sm font-bold uppercase tracking-tighter" title="Rust Documentation">Docs.rs</a>
            </div>
            <p class="text-xs">&copy; 2026 Fabio Falcinelli. Released under the MIT License.</p>
        </footer>`;
    }
}
customElements.define('pinner-footer', PinnerFooter);

// Shared Clipboard Utility
window.copyCmd = function(text, btn) {
    navigator.clipboard.writeText(text).then(() => {
        const feedback = document.getElementById('copy-feedback');
        if (feedback) {
            feedback.classList.remove('opacity-0');
            feedback.classList.add('opacity-100');
            
            setTimeout(() => {
                feedback.classList.remove('opacity-100');
                feedback.classList.add('opacity-0');
            }, 2000);
        }
    });
};
