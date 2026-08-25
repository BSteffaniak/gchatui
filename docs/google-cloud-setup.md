# Google Cloud setup

The first release will use a desktop OAuth client supplied by each user. Detailed
steps will be verified during OAuth implementation. The intended setup is:

1. Create or select a Google Cloud project.
2. Enable the Google Chat API.
3. Configure an OAuth consent screen for the intended test audience.
4. Create a Desktop application OAuth client.
5. Download the client configuration to a private local path.
6. Configure gchatui to read that path without copying the file.

Only read-only Chat scopes will be requested. Organization policy can require an
administrator to approve an OAuth client even though the application does not make
company-wide configuration changes.

Never commit the downloaded client file or authorization output.
